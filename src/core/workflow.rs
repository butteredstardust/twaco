//! The local write loop, `sync`, as a result rather than as printing.
//!
//! The CLI prints the log in order; the MCP server returns it. Both run exactly this, so they
//! cannot disagree about what a sync did. The per-kind steps were moved here from the CLI
//! unchanged, printing turned into log lines.

use super::config::Solution;
use super::lock::WorkspaceLock;
use super::transaction::Transaction;
use super::workspace::EntityFile;
use std::collections::{BTreeMap, BTreeSet};

/// What happened, in the order it happened. Errors are kept apart from changes so a caller can
/// route them (the CLI to stderr) without losing the order within each.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Log {
    pub lines: Vec<Line>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    Change(String),
    Error(String),
}

impl Log {
    fn change(&mut self, line: String) {
        self.lines.push(Line::Change(line));
    }

    fn error(&mut self, line: String) {
        self.lines.push(Line::Error(line));
    }

    pub(crate) fn changes(&self) -> impl Iterator<Item = &str> {
        self.lines.iter().filter_map(|line| match line {
            Line::Change(text) => Some(text.as_str()),
            Line::Error(_) => None,
        })
    }

    pub(crate) fn errors(&self) -> impl Iterator<Item = &str> {
        self.lines.iter().filter_map(|line| match line {
            Line::Error(text) => Some(text.as_str()),
            Line::Change(_) => None,
        })
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SyncOptions {
    /// Report what would change and write nothing.
    pub(crate) check: bool,
    /// Permit a service or field to appear or disappear.
    pub(crate) allow_structural: bool,
    /// Rewrite every script payload in the configured layout.
    pub(crate) relayout: bool,
    /// The entities were asked for by name, so one with no sidecars is a failure rather than
    /// simply not under management.
    pub(crate) named: bool,
}

#[derive(Debug, Default)]
pub struct SyncOutcome {
    pub log: Log,
    /// Entity files with at least one sidecar compared.
    pub checked: usize,
    /// Entity files that changed, or would with `check`.
    pub changed: usize,
    pub failed: usize,
    pub types: super::types::Refresh,
}

#[derive(Default)]
struct Tally {
    /// Something was compared, so this file is under management.
    examined: bool,
    changed: bool,
    managed: bool,
    failed: usize,
}

impl Tally {
    fn record(&mut self, outcome: Result<Option<bool>, ()>) {
        match outcome {
            Ok(Some(did_change)) => {
                self.examined = true;
                self.managed = true;
                self.changed |= did_change;
            }
            // Not under management for this kind. The file may still have sidecars of another.
            Ok(None) => {}
            Err(()) => {
                self.managed = true;
                self.failed += 1;
            }
        }
    }
}

/// Write every sidecar kind back into its entity file: service scripts, DataShape fields,
/// mashup content, DataTable configuration. `unreadable` are files discovery could not read,
/// each already a failure.
pub(crate) fn sync(
    solution: &Solution,
    chosen: &[EntityFile],
    unreadable: &[String],
    options: SyncOptions,
) -> SyncOutcome {
    let mut outcome = SyncOutcome::default();
    let log = &mut outcome.log;
    // A DataShape import makes ThingWorx rewrite every table on that shape, even one in another
    // project, so a DataShape synced here brings its tables along. A table follows the DataShape
    // as this sync leaves it: a chosen one's XML after its own sync (nothing, when that sync is
    // refused), any other's XML as it stands.
    let mut work = chosen.to_vec();
    let mut shapes = BTreeMap::new();
    let tables_or_shapes = chosen.iter().any(|entity| {
        entity.info.collection == "DataShapes"
            || std::fs::read(&entity.path)
                .is_ok_and(|src| super::datatable::shape_and_accumulated(&src).is_some())
    });
    if tables_or_shapes {
        let found = super::workspace::discover(solution).entities;
        let mut synced = BTreeSet::new();
        for entity in found
            .iter()
            .filter(|entity| entity.info.collection == "DataShapes")
        {
            let named = chosen.iter().any(|chosen| chosen.path == entity.path);
            let fields = if named {
                shape_fields_after_sync(solution, entity, options.allow_structural)
            } else {
                std::fs::read(&entity.path)
                    .ok()
                    .and_then(|src| super::datashape::extract(&src).ok())
            };
            if let Some(fields) = fields {
                if named {
                    synced.insert(entity.info.name.clone());
                }
                shapes.insert(entity.info.name.clone(), fields);
            }
        }
        for entity in found {
            if work.iter().any(|chosen| chosen.path == entity.path) {
                continue;
            }
            let Ok(src) = std::fs::read(&entity.path) else {
                continue;
            };
            if super::datatable::shape_and_accumulated(&src)
                .is_some_and(|(shape, _)| synced.contains(&shape))
            {
                work.push(entity);
            }
        }
        work.sort_by(|a, b| a.path.cmp(&b.path));
    }
    for problem in unreadable {
        log.error(problem.clone());
    }
    let mut failed = unreadable.len();
    for entity in &work {
        let src = match std::fs::read(&entity.path) {
            Ok(s) => s,
            Err(e) => {
                log.error(format!("{}: {e}", entity.path.display()));
                failed += 1;
                continue;
            }
        };
        let mut tally = Tally::default();
        // Each kind changes the bytes in memory, in turn, and the file is written once at the
        // end: a crash leaves it as it was or as it should be, never with only some kinds in.
        let mut current = src.clone();

        if entity.info.collection == "DataShapes" {
            tally.record(sync_fields(
                log,
                solution,
                entity,
                &mut current,
                options.check,
                options.allow_structural,
            ));
        }
        if entity.info.collection == "Mashups" {
            tally.record(sync_mashup(
                log,
                solution,
                entity,
                &mut current,
                options.check,
            ));
        }
        // A DataTable whose own datatable.json is refused is left alone: the merge would write
        // both files, overwriting the sidecar the person still has to correct.
        let mut table_refused = false;
        if super::datatable::is_data_table(&src) {
            let synced = sync_datatable(log, solution, entity, &mut current, options.check);
            table_refused = synced.is_err();
            tally.record(synced);
        }
        let mut accumulated_sidecar = None;
        if let Some(fields) = super::datatable::shape_and_accumulated(&current)
            .filter(|_| !table_refused)
            .and_then(|(shape, _)| shapes.get(&shape))
        {
            if sync_accumulated(log, entity, &mut current, fields, options.check) {
                tally.record(Ok(Some(true)));
                if super::datatable::is_data_table(&current) {
                    let path = super::workspace::datatable_path(solution, entity);
                    if path.is_file() {
                        if let Ok(configuration) = super::datatable::extract(&current) {
                            if let Ok(text) = super::datatable::to_sidecar(&configuration) {
                                accumulated_sidecar = Some((path, text));
                            }
                        }
                    }
                }
            }
        }

        let dir = super::workspace::services_dir(solution, entity);
        let sidecars = super::workspace::read_sidecars(&dir);
        // An existing empty services directory is deliberate management of zero services. This
        // matters before Git drops the empty directory: it lets sync report (or, when allowed,
        // remove) the last script service instead of silently treating the entity as unmanaged.
        if !sidecars.is_empty() || dir.is_dir() {
            tally.record(sync_services(
                log,
                entity,
                &mut current,
                &sidecars,
                options,
                solution.format.indent_cdata_payload,
            ));
        } else if !tally.managed && options.named {
            // Under --all, no sidecars means the entity is simply not under management. Asked
            // for by name, it means the command cannot do what was requested, and reporting
            // success for that would be a lie.
            log.error(format!(
                "{} has no sidecars at {}",
                entity.info.name,
                dir.display()
            ));
            failed += 1;
        }
        // What the kinds that succeeded changed is written, as when each wrote its own; a
        // write that fails leaves the file as it was, so nothing counts as changed.
        let mut entity_written = current == src;
        if !options.check && current != src {
            if let Err(e) = super::workspace::write_entity(&entity.path, &current) {
                log.error(format!("{e}"));
                tally.changed = false;
                tally.failed += 1;
            } else {
                entity_written = true;
            }
        }
        if !options.check && entity_written {
            if let Some((path, text)) = accumulated_sidecar {
                if let Err(e) = super::workspace::write_sidecar_if_changed(&path, &text) {
                    log.error(format!("{e}"));
                    tally.failed += 1;
                }
            }
        }

        if tally.examined {
            outcome.checked += 1;
        }
        if tally.changed {
            outcome.changed += 1;
        }
        failed += tally.failed;
    }
    outcome.failed = failed;
    outcome
}

/// A DataShape's fields once its `fields.json` is synced into it, or as its XML holds them when it
/// has none. `None` when the sync would be refused (an add or remove without `allow_structural`,
/// a sidecar that will not parse): its tables must not get ahead of the DataShape.
pub(crate) fn shape_fields_after_sync(
    solution: &Solution,
    entity: &super::workspace::EntityFile,
    allow_structural: bool,
) -> Option<Vec<super::datashape::Field>> {
    let src = std::fs::read(&entity.path).ok()?;
    let path = super::workspace::fields_path(solution, entity);
    let Ok(text) = std::fs::read_to_string(path) else {
        return super::datashape::extract(&src).ok();
    };
    let desired = super::datashape::from_sidecar(&super::workspace::lf(&text)).ok()?;
    let (out, _) = super::datashape::sync(&src, &desired, allow_structural).ok()?;
    super::datashape::extract(&out).ok()
}

/// Apply the server's DataShape-import merge to one table; whether it changed. Failure is
/// intentionally quiet: unknown and malformed accumulated shapes are outside the verified
/// platform contract.
fn sync_accumulated(
    log: &mut Log,
    entity: &super::workspace::EntityFile,
    current: &mut Vec<u8>,
    fields: &[super::datashape::Field],
    check: bool,
) -> bool {
    let Some((shape, old)) = super::datatable::shape_and_accumulated(current) else {
        return false;
    };
    let Some(new) = super::accumulated::merge(&old, fields) else {
        return false;
    };
    let Ok(out) = super::datatable::write_accumulated(current, &new) else {
        return false;
    };
    if out == *current {
        return false;
    }
    log.change(format!(
        "{}: accumulated shape {} {shape}: {}",
        entity.info.name,
        if check { "would follow" } else { "follows" },
        accumulated_changes(&old, fields)
    ));
    *current = out;
    true
}

/// What a merge does to an accumulated shape, in words: fields added, deleted, restored (a
/// deleted field that is back) and retyped.
pub(crate) fn accumulated_changes(old: &str, fields: &[super::datashape::Field]) -> String {
    let old = serde_json::from_str::<serde_json::Value>(old)
        .ok()
        .and_then(|value| value.get("fieldDefinitions").cloned())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let is_deleted = |field: &serde_json::Value| {
        field.pointer("/aspects/isDeleted") == Some(&serde_json::Value::Bool(true))
    };
    let mut added = Vec::new();
    let mut restored = Vec::new();
    let mut retyped = Vec::new();
    for field in fields {
        match old.get(&field.name) {
            None => added.push(field.name.clone()),
            Some(existing) => {
                if is_deleted(existing) {
                    restored.push(field.name.clone());
                }
                if existing.get("baseType").and_then(serde_json::Value::as_str)
                    != Some(&field.base_type)
                {
                    retyped.push(field.name.clone());
                }
            }
        }
    }
    let desired: BTreeSet<&str> = fields.iter().map(|field| field.name.as_str()).collect();
    let mut deleted: Vec<String> = old
        .iter()
        .filter(|(name, field)| !desired.contains(name.as_str()) && !is_deleted(field))
        .map(|(name, _)| name.clone())
        .collect();
    let mut parts = Vec::new();
    for (names, word) in [
        (&mut added, "added"),
        (&mut deleted, "deleted"),
        (&mut restored, "restored"),
        (&mut retyped, "retyped"),
    ] {
        if !names.is_empty() {
            names.sort();
            parts.push(format!("{word} {}", names.join(", ")));
        }
    }
    parts.join("; ")
}

/// Fold one entity's service sidecars into `current`. Returns whether anything changed.
fn sync_services(
    log: &mut Log,
    entity: &super::workspace::EntityFile,
    current: &mut Vec<u8>,
    sidecars: &BTreeMap<String, super::sidecar::ServiceSidecar>,
    options: SyncOptions,
    indent_cdata_payload: bool,
) -> Result<Option<bool>, ()> {
    let check = options.check;
    match super::sync::sync(
        current,
        sidecars,
        options.allow_structural,
        indent_cdata_payload,
        options.relayout,
    ) {
        Ok((out, report)) => {
            if out == *current {
                return Ok(Some(false));
            }
            // A plain edit keeps the one-part message; an add or remove is named as such.
            let structural = report.has_structural_change();
            let mut parts = Vec::new();
            for (names, would, did) in [
                (&report.changed, "would change", "changed"),
                (&report.only_in_sidecars, "would add", "added"),
                (&report.only_in_entity, "would remove", "removed"),
            ] {
                if !names.is_empty() || (!structural && std::ptr::eq(names, &report.changed)) {
                    parts.push(format!(
                        "{} service(s) {}: {}",
                        names.len(),
                        if check { would } else { did },
                        names.join(", ")
                    ));
                }
            }
            if !report.dropped_permissions.is_empty() {
                parts.push(format!(
                    "run-time permissions of the removed service(s) {}: {}",
                    if check { "would go too" } else { "went too" },
                    report.dropped_permissions.join(", ")
                ));
            }
            log.change(format!("{}: {}", entity.info.name, parts.join("; ")));
            *current = out;
            Ok(Some(true))
        }
        Err(e) => {
            log.error(format!("{}: {e}", entity.info.name));
            Err(())
        }
    }
}

/// Fold one DataShape's field sidecar into `current`. Returns whether anything changed.
fn sync_fields(
    log: &mut Log,
    solution: &Solution,
    entity: &super::workspace::EntityFile,
    current: &mut Vec<u8>,
    check: bool,
    allow_structural: bool,
) -> Result<Option<bool>, ()> {
    let path = super::workspace::fields_path(solution, entity);
    let Ok(text) = std::fs::read_to_string(&path) else {
        // No sidecar at all. `None` rather than "nothing changed", so the caller can tell an
        // unmanaged DataShape from one that is already in sync.
        return Ok(None);
    };
    // CRLF to LF. This once read `replace("<LF>", "<LF>")`, a no-op left by a shell
    // heredoc that ate the escapes: harmless only because JSON treats CR as whitespace.
    let desired = match super::datashape::from_sidecar(&super::workspace::lf(&text)) {
        Ok(d) => d,
        Err(e) => {
            log.error(format!("{}: {e}", path.display()));
            return Err(());
        }
    };
    match super::datashape::sync(current, &desired, allow_structural) {
        Ok((out, changes)) => {
            if out == *current {
                return Ok(Some(false));
            }
            log.change(format!(
                "{}: {} field change(s) {}: {}",
                entity.info.name,
                changes.len(),
                if check { "would apply" } else { "applied" },
                changes.join(", ")
            ));
            *current = out;
            Ok(Some(true))
        }
        Err(e) => {
            log.error(format!("{}: {e}", entity.info.name));
            Err(())
        }
    }
}

/// Fold one mashup's sidecars into `current`. Returns whether anything changed.
fn sync_mashup(
    log: &mut Log,
    solution: &Solution,
    entity: &super::workspace::EntityFile,
    current: &mut Vec<u8>,
    check: bool,
) -> Result<Option<bool>, ()> {
    let dir = super::workspace::mashup_dir(solution, entity);
    let assets = match super::workspace::read_mashup(&dir) {
        // No content.json: this mashup is simply not under management.
        Ok(None) => return Ok(None),
        Ok(Some(assets)) => assets,
        Err(e) => {
            log.error(format!("{e}"));
            return Err(());
        }
    };
    match super::mashup::sync(current, &assets) {
        Ok((out, changes)) => {
            if out == *current {
                return Ok(Some(false));
            }
            log.change(format!(
                "{}: {} {}: {}",
                entity.info.name,
                changes.len(),
                if check { "would change" } else { "changed" },
                changes.join(", ")
            ));
            *current = out;
            Ok(Some(true))
        }
        Err(e) => {
            log.error(format!("{}: {e}", entity.info.name));
            Err(())
        }
    }
}

/// Fold one DataTable's configuration sidecar into `current`.
fn sync_datatable(
    log: &mut Log,
    solution: &Solution,
    entity: &super::workspace::EntityFile,
    current: &mut Vec<u8>,
    check: bool,
) -> Result<Option<bool>, ()> {
    let path = super::workspace::datatable_path(solution, entity);
    let text = match super::workspace::read_datatable(&path) {
        Ok(None) => return Ok(None),
        Ok(Some(text)) => text,
        Err(e) => {
            log.error(format!("{e}"));
            return Err(());
        }
    };
    let desired = match super::datatable::from_sidecar(&text) {
        Ok(d) => d,
        Err(e) => {
            log.error(format!("{}: {e}", path.display()));
            return Err(());
        }
    };
    match super::datatable::sync(current, &desired) {
        Ok((out, changes)) => {
            if out == *current {
                return Ok(Some(false));
            }
            log.change(format!(
                "{}: {} configuration change(s) {}: {}",
                entity.info.name,
                changes.len(),
                if check { "would apply" } else { "applied" },
                changes.join(", ")
            ));
            *current = out;
            Ok(Some(true))
        }
        Err(e) => {
            log.error(format!("{}: {e}", entity.info.name));
            Err(())
        }
    }
}

#[derive(Debug, Default)]
pub struct ExtractOutcome {
    pub log: Log,
    /// Sidecar parts written: services, fields, a mashup's two files, a DataTable's one.
    pub written: usize,
    /// Entity files that yielded at least one part.
    pub entities: usize,
    pub failed: usize,
    pub types: super::types::Refresh,
}

/// Entity XML to sidecars, every kind, for the entities chosen. `named` means they were asked
/// for by name, so one with nothing to extract is worth a line.
///
/// Each entity's sidecars are written as one transaction under `lock`: after a failure or a
/// crash they are all as they were or all as the entity has them, never one service's new
/// definition beside its old script. A kind that cannot be read from the entity is reported and
/// the others are still written, as are the other entities. A file that already holds what it
/// should is not written.
pub(crate) fn extract(
    solution: &Solution,
    chosen: &[EntityFile],
    unreadable: &[String],
    named: bool,
    lock: &WorkspaceLock,
) -> ExtractOutcome {
    let mut outcome = ExtractOutcome::default();
    let mut written = 0usize;
    let mut entities = 0usize;
    let mut failed = unreadable.len();
    let log = &mut outcome.log;
    for problem in unreadable {
        log.error(problem.to_string());
    }

    for entity in chosen {
        let src = match std::fs::read(&entity.path) {
            Ok(s) => s,
            Err(e) => {
                log.error(format!("{}: {e}", entity.path.display()));
                failed += 1;
                continue;
            }
        };
        let mut transaction = Transaction::new(&solution.root, "extract");
        // Said, and counted, once the entity's sidecars are written.
        let mut lines: Vec<String> = Vec::new();
        let mut parts = 0usize;
        // A file of it that cannot be planned: then none of its sidecars are written, rather
        // than some of one service's.
        let mut unplanned = false;

        // A DataShape's fields are a sidecar of their own. Not `continue`: a DataShape may
        // also declare services, and returning early here would silently skip them.
        if entity.info.collection == "DataShapes" {
            match super::datashape::extract(&src) {
                Ok(fields) => {
                    let path = super::workspace::fields_path(solution, entity);
                    let text = super::datashape::to_sidecar(&fields);
                    match plan_lf(&mut transaction, &path, &text) {
                        Ok(()) => {
                            parts += fields.len();
                            lines.push(format!(
                                "{}: {} field(s) -> {}",
                                entity.info.name,
                                fields.len(),
                                path.display()
                            ));
                        }
                        Err(e) => {
                            log.error(format!("{e}"));
                            unplanned = true;
                        }
                    }
                }
                Err(e) => {
                    log.error(format!("{}: {e}", entity.path.display()));
                    failed += 1;
                }
            }
        }
        // A mashup's payload is one JSON blob with a stylesheet buried inside it, and both come
        // out as files a person can edit.
        if entity.info.collection == "Mashups" {
            match super::mashup::extract(&src) {
                Ok(assets) => {
                    let dir = super::workspace::mashup_dir(solution, entity);
                    match plan_lf(&mut transaction, &dir.join("content.json"), &assets.content)
                        .and_then(|()| {
                            plan_lf(&mut transaction, &dir.join("custom.css"), &assets.css)
                        }) {
                        Ok(()) => {
                            parts += 2;
                            lines.push(format!(
                                "{}: content and stylesheet -> {}",
                                entity.info.name,
                                dir.display()
                            ));
                        }
                        Err(e) => {
                            log.error(format!("{e}"));
                            unplanned = true;
                        }
                    }
                }
                Err(e) => {
                    log.error(format!("{}: {e}", entity.path.display()));
                    failed += 1;
                }
            }
        }

        // A DataTable keeps its shape and indexes in configuration tables rather than fields.
        if super::datatable::is_data_table(&src) {
            match super::datatable::extract(&src)
                .and_then(|c| super::datatable::to_sidecar(&c).map(|t| (c, t)))
            {
                Ok((configuration, text)) => {
                    let path = super::workspace::datatable_path(solution, entity);
                    match plan_lf(&mut transaction, &path, &text) {
                        Ok(()) => {
                            parts += 1;
                            lines.push(format!(
                                "{}: {} index(es) and a shape -> {}",
                                entity.info.name,
                                configuration.indexes.len(),
                                path.display()
                            ));
                        }
                        Err(e) => {
                            log.error(format!("{e}"));
                            unplanned = true;
                        }
                    }
                }
                Err(e) => {
                    log.error(format!("{}: {e}", entity.path.display()));
                    failed += 1;
                }
            }
        }

        match super::sidecar::extract(&src) {
            Ok(extraction) if extraction.services.is_empty() => {
                // Naming an entity with nothing to extract is worth saying; sweeping past it
                // under --all is not, and neither is one whose fields or mashup came out.
                if named && lines.is_empty() {
                    lines.push(format!("{}: no script services", entity.info.name));
                    skipped_lines(&mut lines, &extraction);
                }
            }
            Ok(extraction) => {
                let dir = super::workspace::services_dir(solution, entity);
                let planned = extraction.services.iter().try_for_each(|service| {
                    let service_dir = dir.join(&service.name);
                    plan_lf(
                        &mut transaction,
                        &service_dir.join("definition.xml"),
                        &service.definition,
                    )?;
                    plan_lf(
                        &mut transaction,
                        &service_dir.join("script.js"),
                        &service.script,
                    )
                });
                match planned {
                    Ok(()) => {
                        parts += extraction.services.len();
                        lines.push(format!(
                            "{}: {} service(s) -> {}",
                            entity.info.name,
                            extraction.services.len(),
                            dir.display()
                        ));
                        skipped_lines(&mut lines, &extraction);
                        let stale = super::workspace::stale_sidecars(&dir, &extraction.services);
                        if !stale.is_empty() {
                            lines
                                .push(format!("    no longer in the entity: {}", stale.join(", ")));
                        }
                    }
                    Err(e) => {
                        log.error(format!("{e}"));
                        unplanned = true;
                    }
                }
            }
            Err(e) => {
                log.error(format!("{}: {e}", entity.path.display()));
                failed += 1;
            }
        }

        if unplanned {
            log.error(format!(
                "{}: none of its sidecars were written",
                entity.info.name
            ));
            failed += 1;
            continue;
        }
        if !transaction.is_empty() {
            if let Err(e) = transaction.apply(lock) {
                log.error(format!("{}: {e}", entity.info.name));
                failed += 1;
                continue;
            }
        }
        for line in lines {
            log.change(line);
        }
        if parts > 0 {
            entities += 1;
            written += parts;
        }
    }
    outcome.written = written;
    outcome.entities = entities;
    outcome.failed = failed;
    outcome.types = super::types::refresh_after_write(solution, written > 0, lock);
    outcome
}

/// Plan one sidecar file, which twaco writes with LF line endings.
fn plan_lf(
    transaction: &mut Transaction,
    path: &std::path::Path,
    text: &str,
) -> Result<(), super::transaction::TransactionError> {
    transaction.write_file(
        path,
        super::workspace::keeping_bom(path, text.replace("\r\n", "\n").into_bytes()),
    )
}

/// Say what an entity held that did not become a sidecar, so it is never a silent omission.
fn skipped_lines(lines: &mut Vec<String>, extraction: &super::sidecar::Extraction) {
    if !extraction.non_script.is_empty() {
        lines.push(format!(
            "    not scripts: {}",
            extraction.non_script.join(", ")
        ));
    }
    if !extraction.inherited.is_empty() {
        lines.push(format!(
            "    defined elsewhere: {}",
            extraction.inherited.join(", ")
        ));
    }
    if !extraction.without_script.is_empty() {
        lines.push(format!(
            "    no implementation: {}",
            extraction.without_script.join(", ")
        ));
    }
}

#[derive(Debug, Default)]
pub struct FmtOutcome {
    pub log: Log,
    /// Service scripts looked at.
    pub files: usize,
    /// Scripts reformatted, or that would be with `check`.
    pub changed: Vec<std::path::PathBuf>,
    pub failed: usize,
}

/// Format every service script sidecar with the built-in formatter.
///
/// A reformatted script is written through the workspace's atomic write, as every other file
/// twaco writes is. Until this moved here it used a plain `fs::write`, so an interrupted run
/// could leave a script truncated.
pub(crate) fn fmt(solution: &Solution, check: bool) -> FmtOutcome {
    let mut outcome = FmtOutcome::default();
    let style = super::fmt::Style::default();
    let tree = super::workspace::script_files(solution);
    for why in &tree.unreadable {
        outcome
            .log
            .error(format!("not formatted, could not be read: {why}"));
        outcome.failed += 1;
    }
    let files = tree.files;
    outcome.files = files.len();
    for path in &files {
        let source = match std::fs::read_to_string(path) {
            Ok(s) => s.replace("\r\n", "\n"),
            Err(e) => {
                outcome.log.error(format!("{}: {e}", path.display()));
                outcome.failed += 1;
                continue;
            }
        };
        match super::fmt::format(&source, &style) {
            Ok(None) => {}
            Ok(Some(formatted)) => {
                if check {
                    outcome.changed.push(path.clone());
                } else if let Err(e) = super::workspace::write_entity(path, formatted.as_bytes()) {
                    outcome.log.error(e.to_string());
                    outcome.failed += 1;
                } else {
                    // Only counted once the write succeeded, so nothing claims to have
                    // reformatted a file it could not touch.
                    outcome.changed.push(path.clone());
                }
            }
            Err(e) => {
                outcome.log.error(format!("{}: {e}", path.display()));
                outcome.failed += 1;
            }
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{config::Solution, types, workspace};
    use std::path::PathBuf;

    #[test]
    fn a_byte_order_mark_is_a_sidecar_encoding_not_script_content() {
        let (_dir, root, solution) = service_fixture("bom");
        let script = root.join("src/T/services/Run/script.js");
        // An editor saved the unchanged script with a byte-order mark: nothing to sync.
        std::fs::write(&script, "\u{feff}old();").unwrap();
        let checked = sync(
            &solution,
            &entities(&solution),
            &[],
            SyncOptions {
                check: true,
                ..SyncOptions::default()
            },
        );
        assert_eq!(checked.changed, 0, "{:?}", checked.log);

        // A real change syncs without the mark reaching the entity.
        std::fs::write(&script, "\u{feff}changed();").unwrap();
        let synced = sync(&solution, &entities(&solution), &[], SyncOptions::default());
        assert_eq!(synced.changed, 1, "{:?}", synced.log);
        let xml = std::fs::read_to_string(root.join("Things/T.xml")).unwrap();
        assert!(xml.contains("changed();"), "{xml}");
        assert!(!xml.contains('\u{feff}'), "{xml}");

        // Extract rewrites the script from the entity and keeps the mark the file had.
        let xml = xml.replace("changed();", "again();");
        std::fs::write(root.join("Things/T.xml"), xml).unwrap();
        let extracted = extract(
            &solution,
            &entities(&solution),
            &[],
            false,
            &crate::core::lock::acquire(&solution.root, "test", &[]).unwrap(),
        );
        assert!(extracted.written > 0);
        assert_eq!(
            std::fs::read_to_string(&script).unwrap(),
            "\u{feff}again();"
        );
    }

    #[test]
    fn only_a_leading_byte_order_mark_is_encoding() {
        assert_eq!(crate::core::workspace::lf("\u{feff}a\r\nb"), "a\nb");
        assert_eq!(crate::core::workspace::lf("a\u{feff}b"), "a\u{feff}b");
        assert_eq!(crate::core::workspace::lf("\u{feff}\u{feff}a"), "\u{feff}a");
    }

    fn service_fixture(label: &str) -> (tempfile::TempDir, PathBuf, Solution) {
        let root_guard = tempfile::Builder::new()
            .prefix(&format!("twaco-workflow-{label}-"))
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        std::fs::create_dir_all(root.join("Things")).unwrap();
        std::fs::create_dir_all(root.join("src/T/services/Run")).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        std::fs::write(
            root.join("Things/T.xml"),
            concat!(
                "<Entities><Things><Thing name=\"T\" projectName=\"P\"><ThingShape>",
                "<ServiceDefinitions><ServiceDefinition name=\"Run\"><ParameterDefinitions></ParameterDefinitions>",
                "<ResultType baseType=\"NOTHING\"/></ServiceDefinition></ServiceDefinitions>",
                "<ServiceImplementations><ServiceImplementation name=\"Run\" handlerName=\"Script\">",
                "<ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row>",
                "<code><![CDATA[old();]]></code>",
                "</Row></Rows></ConfigurationTable></ConfigurationTables>",
                "</ServiceImplementation></ServiceImplementations>",
                "</ThingShape></Thing></Things></Entities>"
            ),
        ).unwrap();
        std::fs::write(root.join("src/T/services/Run/script.js"), "old();").unwrap();
        std::fs::write(
            root.join("src/T/services/Run/definition.xml"),
            "<ServiceDefinition name=\"Run\"><ParameterDefinitions></ParameterDefinitions><ResultType baseType=\"NOTHING\"/></ServiceDefinition>\n",
        ).unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (root_guard, root, solution)
    }

    fn entities(solution: &Solution) -> Vec<EntityFile> {
        workspace::discover(solution).entities
    }

    #[test]
    fn an_empty_services_directory_manages_the_last_service() {
        let (_dir, root, solution) = service_fixture("empty-services");
        std::fs::remove_dir_all(root.join("src/T/services/Run")).unwrap();
        std::fs::create_dir_all(root.join("src/T/services")).unwrap();
        let refused = sync(&solution, &entities(&solution), &[], SyncOptions::default());
        assert_eq!(refused.failed, 1, "{:?}", refused.log);
        assert!(read(&root, "Things/T.xml").contains("name=\"Run\""));
        let applied = sync(
            &solution,
            &entities(&solution),
            &[],
            SyncOptions {
                allow_structural: true,
                ..SyncOptions::default()
            },
        );
        assert_eq!(applied.failed, 0, "{:?}", applied.log);
        assert!(!read(&root, "Things/T.xml").contains("name=\"Run\""));
    }

    /// A DataShape P.D (id, n) with its fields.json, a DataTable P.Rows with its datatable.json,
    /// a Stream P.Log, and a Stream P.Other on a DataShape outside the solution, all settled.
    fn shape_fixture(label: &str) -> (tempfile::TempDir, PathBuf, Solution) {
        let root_guard = tempfile::Builder::new()
            .prefix(&format!("twaco-workflow-{label}-"))
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        for dir in ["DataShapes", "Things", "src/P.D", "src/P.Rows"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        let shape = "<Entities><DataShapes><DataShape name=\"P.D\" projectName=\"P\"><FieldDefinitions>\
            <FieldDefinition aspect.isPrimaryKey=\"true\" baseType=\"STRING\" description=\"\" name=\"id\" ordinal=\"1\"></FieldDefinition>\
            <FieldDefinition aspect.isPrimaryKey=\"false\" baseType=\"INTEGER\" description=\"\" name=\"n\" ordinal=\"2\"></FieldDefinition>\
            </FieldDefinitions></DataShape></DataShapes></Entities>";
        std::fs::write(root.join("DataShapes/P.D.xml"), shape).unwrap();
        let fields = super::super::datashape::extract(shape.as_bytes()).unwrap();
        std::fs::write(
            root.join("src/P.D/fields.json"),
            super::super::datashape::to_sidecar(&fields),
        )
        .unwrap();
        let accumulated = super::super::accumulated::accumulated_json(&fields);
        let thing = |name: &str, template: &str, shape: &str| {
            format!("<Entities><Things><Thing name=\"{name}\" projectName=\"P\" thingTemplate=\"{template}\"><ConfigurationTables><ConfigurationTable name=\"Settings\"><Rows><Row><accumulatedDataShape><json><![CDATA[{accumulated}]]></json></accumulatedDataShape><dataShape><![CDATA[{shape}]]></dataShape></Row></Rows></ConfigurationTable></ConfigurationTables></Thing></Things></Entities>")
        };
        let rows = thing("P.Rows", "DataTable", "P.D");
        std::fs::write(root.join("Things/P.Rows.xml"), &rows).unwrap();
        std::fs::write(
            root.join("src/P.Rows/datatable.json"),
            super::super::datatable::to_sidecar(
                &super::super::datatable::extract(rows.as_bytes()).unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            root.join("Things/P.Log.xml"),
            thing("P.Log", "Stream", "P.D"),
        )
        .unwrap();
        std::fs::write(
            root.join("Things/P.Other.xml"),
            thing("P.Other", "Stream", "Elsewhere.D"),
        )
        .unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (root_guard, root, solution)
    }

    /// Add a field `extra` to P.D's fields.json.
    fn add_extra_field(root: &std::path::Path) {
        let path = root.join("src/P.D/fields.json");
        let mut fields = super::super::datashape::from_sidecar(
            &std::fs::read_to_string(&path)
                .unwrap()
                .replace("\r\n", "\n"),
        )
        .unwrap();
        let mut extra = fields[1].clone();
        extra.name = "extra".into();
        extra.base_type = "STRING".into();
        extra.ordinal = "3".into();
        fields.push(extra);
        std::fs::write(&path, super::super::datashape::to_sidecar(&fields)).unwrap();
    }

    fn named(solution: &Solution, names: &[&str]) -> Vec<EntityFile> {
        entities(solution)
            .into_iter()
            .filter(|entity| names.contains(&entity.info.name.as_str()))
            .collect()
    }

    fn read(root: &std::path::Path, path: &str) -> String {
        std::fs::read_to_string(root.join(path)).unwrap()
    }

    const EXTRA: &str = r#""extra":{"name":"extra","aspects":{"isPrimaryKey":false},"description":"","baseType":"STRING","ordinal":2}"#;

    #[test]
    fn a_datashape_change_reaches_its_tables_and_streams_and_settles() {
        let (_dir, root, solution) = shape_fixture("ripple");
        let other = read(&root, "Things/P.Other.xml");
        add_extra_field(&root);
        let options = SyncOptions {
            allow_structural: true,
            ..SyncOptions::default()
        };

        let planned = sync(
            &solution,
            &entities(&solution),
            &[],
            SyncOptions {
                check: true,
                ..options
            },
        );
        assert_eq!(planned.changed, 3, "{:?}", planned.log);
        assert!(planned
            .log
            .changes()
            .any(|line| line == "P.Log: accumulated shape would follow P.D: added extra"));
        assert!(!read(&root, "Things/P.Log.xml").contains("extra"));

        let written = sync(&solution, &entities(&solution), &[], options);
        assert_eq!(written.changed, 3, "{:?}", written.log);
        assert!(read(&root, "Things/P.Rows.xml").contains(EXTRA));
        assert!(read(&root, "Things/P.Log.xml").contains(EXTRA));
        let sidecar = read(&root, "src/P.Rows/datatable.json");
        assert!(sidecar.contains("\"extra\": {"), "{sidecar}");
        assert_eq!(read(&root, "Things/P.Other.xml"), other);

        let again = sync(&solution, &entities(&solution), &[], options);
        assert_eq!(again.changed, 0, "{:?}", again.log);
    }

    #[test]
    fn a_datashape_named_alone_brings_its_tables_along() {
        let (_dir, root, solution) = shape_fixture("named");
        add_extra_field(&root);
        let outcome = sync(
            &solution,
            &named(&solution, &["P.D"]),
            &[],
            SyncOptions {
                allow_structural: true,
                named: true,
                ..SyncOptions::default()
            },
        );
        assert_eq!(outcome.failed, 0, "{:?}", outcome.log);
        assert!(read(&root, "Things/P.Rows.xml").contains(EXTRA));
        assert!(read(&root, "src/P.Rows/datatable.json").contains("\"extra\": {"));
        assert!(read(&root, "Things/P.Log.xml").contains(EXTRA));
    }

    #[test]
    fn a_refused_datashape_sync_leaves_its_tables_alone() {
        let (_dir, root, solution) = shape_fixture("refused");
        let rows = read(&root, "Things/P.Rows.xml");
        add_extra_field(&root);
        // Adding a field needs allow_structural; without it the DataShape keeps its fields, and so
        // must its tables.
        let outcome = sync(&solution, &entities(&solution), &[], SyncOptions::default());
        assert_eq!(outcome.failed, 1, "{:?}", outcome.log);
        assert_eq!(read(&root, "Things/P.Rows.xml"), rows);
        assert!(!read(&root, "Things/P.Log.xml").contains("extra"));
        assert!(!read(&root, "src/P.Rows/datatable.json").contains("extra"));
    }

    #[test]
    fn a_table_whose_own_sidecar_is_refused_is_not_written() {
        let (_dir, root, solution) = shape_fixture("broken-sidecar");
        let broken = read(&root, "src/P.Rows/datatable.json").replace("\"indexes\"", "\"indexez\"");
        std::fs::write(root.join("src/P.Rows/datatable.json"), &broken).unwrap();
        let rows = read(&root, "Things/P.Rows.xml");
        add_extra_field(&root);
        let outcome = sync(
            &solution,
            &entities(&solution),
            &[],
            SyncOptions {
                allow_structural: true,
                ..SyncOptions::default()
            },
        );
        assert_eq!(outcome.failed, 1, "{:?}", outcome.log);
        assert_eq!(read(&root, "src/P.Rows/datatable.json"), broken);
        assert_eq!(read(&root, "Things/P.Rows.xml"), rows);
        // The Stream on the same shape is not held back by the DataTable's sidecar.
        assert!(read(&root, "Things/P.Log.xml").contains(EXTRA));
    }

    #[test]
    fn a_datashape_reaches_tables_of_another_project() {
        let (_dir, root, _) = shape_fixture("projects");
        std::fs::write(
            root.join("twaco.toml"),
            "[[project]]\nname = \"P\"\n\n[[project]]\nname = \"Q\"\n",
        )
        .unwrap();
        let log = read(&root, "Things/P.Log.xml").replace("projectName=\"P\"", "projectName=\"Q\"");
        std::fs::write(root.join("Things/P.Log.xml"), log).unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        add_extra_field(&root);
        // What `sync --project P P.D` chooses: the DataShape alone.
        let chosen: Vec<EntityFile> = named(&solution, &["P.D"])
            .into_iter()
            .filter(|entity| entity.found_under == "P")
            .collect();
        assert_eq!(chosen.len(), 1);
        sync(
            &solution,
            &chosen,
            &[],
            SyncOptions {
                allow_structural: true,
                named: true,
                ..SyncOptions::default()
            },
        );
        assert!(read(&root, "Things/P.Log.xml").contains(EXTRA));
    }

    #[test]
    fn a_table_named_alone_follows_its_datashape_as_it_stands() {
        let (_dir, root, solution) = shape_fixture("table");
        let stale = read(&root, "Things/P.Log.xml");
        add_extra_field(&root);
        sync(
            &solution,
            &named(&solution, &["P.D"]),
            &[],
            SyncOptions {
                allow_structural: true,
                named: true,
                ..SyncOptions::default()
            },
        );
        // The DataShape now has the field; put the Stream back as it was before.
        assert!(read(&root, "Things/P.Log.xml").contains(EXTRA));
        std::fs::write(root.join("Things/P.Log.xml"), &stale).unwrap();

        let outcome = sync(
            &solution,
            &named(&solution, &["P.Log"]),
            &[],
            SyncOptions {
                named: true,
                ..SyncOptions::default()
            },
        );
        assert_eq!(outcome.changed, 1, "{:?}", outcome.log);
        assert!(read(&root, "Things/P.Log.xml").contains(EXTRA));
    }

    #[test]
    fn sync_refreshes_only_after_a_real_write_and_only_when_opted_in() {
        let (_dir, root, solution) = service_fixture("sync-types");
        types::write(
            &solution,
            &crate::core::lock::acquire_for(&solution, "types test").unwrap(),
        )
        .unwrap();
        let globals = root.join("src/T/services/Run/twaco-globals.d.ts");
        std::fs::write(
            root.join("src/T/services/Run/definition.xml"),
            "<ServiceDefinition name=\"Run\"><ParameterDefinitions><FieldDefinition name=\"added\" baseType=\"STRING\"/></ParameterDefinitions><ResultType baseType=\"NOTHING\"/></ServiceDefinition>\n",
        ).unwrap();
        std::fs::remove_file(&globals).unwrap();

        let checked = sync(
            &solution,
            &entities(&solution),
            &[],
            SyncOptions {
                check: true,
                ..SyncOptions::default()
            },
        );
        assert_eq!(checked.changed, 1);
        assert!(checked.types.files_written.is_none());
        assert!(!globals.exists());

        let written = applied_sync(&solution);
        assert_eq!(written.changed, 1);
        assert!(written.types.files_written.is_some());
        assert!(std::fs::read_to_string(&globals)
            .unwrap()
            .contains("declare let added: string;"));

        std::fs::remove_file(&globals).unwrap();
        let unchanged = applied_sync(&solution);
        assert_eq!(unchanged.changed, 0);
        assert!(unchanged.types.files_written.is_none());
        assert!(!globals.exists());

        let (_dir, root, solution) = service_fixture("sync-no-types");
        std::fs::write(root.join("src/T/services/Run/script.js"), "changed();").unwrap();
        let outcome = applied_sync(&solution);
        assert_eq!(outcome.changed, 1);
        assert!(!root.join(".twaco/types").exists());
    }

    /// `twaco sync --all --apply`: the command holds the lock, so it refreshes the declarations.
    fn applied_sync(solution: &Solution) -> SyncOutcome {
        use crate::core::commands::sync::{execute, SyncRequest, SyncTarget};
        let request = SyncRequest {
            target: SyncTarget {
                project: None,
                entities: Vec::new(),
                all: true,
                reject_entities_with_all: false,
                missing_target: "give --all",
            },
            mode: crate::core::commands::Mode::Apply,
            allow_structural: false,
            relayout: false,
            lock_label: "test sync",
        };
        execute(
            solution,
            &request,
            &mut crate::core::commands::Notices::default(),
        )
        .unwrap()
        .report
    }

    fn two_services(name: &str, a: &str) -> String {
        let service = |service: &str, script: &str| {
            (
                format!("<ServiceDefinition name=\"{service}\"><ParameterDefinitions></ParameterDefinitions><ResultType baseType=\"NOTHING\"/></ServiceDefinition>"),
                format!("<ServiceImplementation name=\"{service}\" handlerName=\"Script\"><ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row><code><![CDATA[{script}]]></code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation>"),
            )
        };
        let (a_definition, a_implementation) = service("A", a);
        let (b_definition, b_implementation) = service("B", "b();");
        format!(
            "<Entities><Things><Thing name=\"{name}\" projectName=\"P\"><ThingShape>\
             <ServiceDefinitions>{a_definition}{b_definition}</ServiceDefinitions>\
             <ServiceImplementations>{a_implementation}{b_implementation}</ServiceImplementations>\
             </ThingShape></Thing></Things></Entities>"
        )
    }

    #[test]
    fn an_entitys_sidecars_are_written_together_or_not_at_all() {
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-workflow-together-")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        std::fs::create_dir_all(root.join("Things")).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        std::fs::write(root.join("Things/T.xml"), two_services("T", "old();")).unwrap();
        std::fs::write(root.join("Things/U.xml"), two_services("U", "old();")).unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let lock = crate::core::lock::acquire(&solution.root, "test", &[]).unwrap();
        let first = extract(&solution, &entities(&solution), &[], false, &lock);
        assert_eq!((first.entities, first.failed), (2, 0), "{:?}", first.log);

        // Nothing changed, so nothing is written again.
        let script = root.join("src/T/services/A/script.js");
        let stamp = || std::fs::metadata(&script).unwrap().modified().unwrap();
        let before = stamp();
        let again = extract(&solution, &entities(&solution), &[], false, &lock);
        assert_eq!((again.entities, again.failed), (2, 0), "{:?}", again.log);
        assert_eq!(stamp(), before);

        // A's script changed in both entities, but in T a file sits where B's folder must go.
        std::fs::write(root.join("Things/T.xml"), two_services("T", "new();")).unwrap();
        std::fs::write(root.join("Things/U.xml"), two_services("U", "new();")).unwrap();
        std::fs::remove_dir_all(root.join("src/T/services/B")).unwrap();
        std::fs::write(root.join("src/T/services/B"), "in the way").unwrap();
        let outcome = extract(&solution, &entities(&solution), &[], false, &lock);
        assert_eq!(
            (outcome.entities, outcome.failed),
            (1, 1),
            "{:?}",
            outcome.log
        );
        assert_eq!(
            std::fs::read_to_string(&script).unwrap(),
            "old();",
            "T is as it was"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("src/U/services/A/script.js")).unwrap(),
            "new();",
            "U is extracted all the same"
        );
        assert_eq!(outcome.log.changes().count(), 1, "{:?}", outcome.log);
        drop(lock);
    }

    #[test]
    fn every_kind_of_sidecar_an_entity_has_lands_in_its_one_write() {
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-workflow-kinds-")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        std::fs::create_dir_all(root.join("DataShapes")).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        std::fs::write(
            root.join("DataShapes/D.xml"),
            concat!(
                "<Entities><DataShapes><DataShape name=\"D\" projectName=\"P\">",
                "<FieldDefinitions><FieldDefinition baseType=\"STRING\" description=\"before\" name=\"A\" ordinal=\"1\"></FieldDefinition></FieldDefinitions>",
                "<ServiceDefinitions><ServiceDefinition name=\"Run\"><ParameterDefinitions></ParameterDefinitions>",
                "<ResultType baseType=\"NOTHING\"/></ServiceDefinition></ServiceDefinitions>",
                "<ServiceImplementations><ServiceImplementation name=\"Run\" handlerName=\"Script\">",
                "<ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row>",
                "<code><![CDATA[old();]]></code>",
                "</Row></Rows></ConfigurationTable></ConfigurationTables>",
                "</ServiceImplementation></ServiceImplementations>",
                "</DataShape></DataShapes></Entities>"
            ),
        )
        .unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let extracted = extract(
            &solution,
            &entities(&solution),
            &[],
            false,
            &crate::core::lock::acquire(&solution.root, "test", &[]).unwrap(),
        );
        assert_eq!(extracted.failed, 0, "{:?}", extracted.log);
        let fields = root.join("src/D/fields.json");
        let text = std::fs::read_to_string(&fields).unwrap();
        std::fs::write(&fields, text.replace("before", "after")).unwrap();
        std::fs::write(root.join("src/D/services/Run/script.js"), "changed();").unwrap();

        let outcome = sync(&solution, &entities(&solution), &[], SyncOptions::default());
        assert_eq!(
            (outcome.changed, outcome.failed),
            (1, 0),
            "{:?}",
            outcome.log
        );
        assert_eq!(outcome.log.changes().count(), 2, "{:?}", outcome.log);
        // Each kind starts from what the one before it made, so neither undoes the other.
        let written = std::fs::read_to_string(root.join("DataShapes/D.xml")).unwrap();
        assert!(written.contains("description=\"after\""), "{written}");
        assert!(written.contains("changed();"), "{written}");
        let again = sync(&solution, &entities(&solution), &[], SyncOptions::default());
        assert_eq!(again.changed, 0, "{:?}", again.log);
    }

    #[test]
    fn extract_adds_the_service_type_project_and_refresh_failure_is_advisory() {
        let (_dir, root, solution) = service_fixture("extract-types");
        std::fs::remove_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join(".twaco/types")).unwrap();
        let outcome = extract(
            &solution,
            &entities(&solution),
            &[],
            false,
            &crate::core::lock::acquire(&solution.root, "test", &[]).unwrap(),
        );
        assert!(outcome.written > 0);
        assert!(root.join("src/T/services/Run/jsconfig.json").is_file());
        assert!(outcome.types.warning.is_none());

        let (_dir, root, solution) = service_fixture("extract-no-types");
        std::fs::remove_dir_all(root.join("src")).unwrap();
        let outcome = extract(
            &solution,
            &entities(&solution),
            &[],
            false,
            &crate::core::lock::acquire(&solution.root, "test", &[]).unwrap(),
        );
        assert!(outcome.written > 0);
        assert!(outcome.types.files_written.is_none());
        assert!(!root.join(".twaco/types").exists());
        assert!(!root.join("src/T/services/Run/jsconfig.json").exists());

        let (_dir, root, solution) = service_fixture("refresh-warning");
        std::fs::create_dir_all(root.join(".twaco")).unwrap();
        std::fs::write(root.join(".twaco/types"), "not a directory").unwrap();
        std::fs::write(root.join("src/T/services/Run/script.js"), "changed();").unwrap();
        let outcome = applied_sync(&solution);
        assert_eq!(outcome.changed, 1);
        assert_eq!(outcome.failed, 0);
        assert!(outcome.types.warning.is_some());
        assert!(std::fs::read_to_string(root.join("Things/T.xml"))
            .unwrap()
            .contains("changed();"));
    }

    #[test]
    fn a_data_table_keeps_its_configuration_when_a_service_syncs_after_it() {
        // Two kinds of sidecar write into the one entity file. The second must start from what
        // the first wrote, not from the file as it was before either.
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-workflow-dt-")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        std::fs::create_dir_all(root.join("Things")).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        std::fs::write(
            root.join("Things/T.xml"),
            concat!(
                "<Entities><Things><Thing name=\"T\" projectName=\"P\" thingTemplate=\"DataTable\">",
                "<ConfigurationTables><ConfigurationTable name=\"Settings\"><Rows><Row>",
                "<dataShape><![CDATA[Old_DS]]></dataShape>",
                "</Row></Rows></ConfigurationTable></ConfigurationTables>",
                "<ThingShape><ServiceDefinitions><ServiceDefinition name=\"Run\"><ParameterDefinitions></ParameterDefinitions>",
                "<ResultType baseType=\"NOTHING\"/></ServiceDefinition></ServiceDefinitions>",
                "<ServiceImplementations><ServiceImplementation name=\"Run\" handlerName=\"Script\">",
                "<ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row>",
                "<code><![CDATA[old();]]></code>",
                "</Row></Rows></ConfigurationTable></ConfigurationTables>",
                "</ServiceImplementation></ServiceImplementations>",
                "</ThingShape></Thing></Things></Entities>"
            ),
        )
        .unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let extracted = extract(
            &solution,
            &entities(&solution),
            &[],
            false,
            &crate::core::lock::acquire(&solution.root, "test", &[]).unwrap(),
        );
        assert_eq!(extracted.failed, 0);

        let table = root.join("src/T/datatable.json");
        let text = std::fs::read_to_string(&table)
            .unwrap()
            .replace("Old_DS", "New_DS");
        std::fs::write(&table, text).unwrap();
        std::fs::write(root.join("src/T/services/Run/script.js"), "changed();").unwrap();

        let outcome = sync(&solution, &entities(&solution), &[], SyncOptions::default());
        assert_eq!(outcome.failed, 0);
        let xml = std::fs::read_to_string(root.join("Things/T.xml")).unwrap();
        assert!(xml.contains("changed();"), "the script synced: {xml}");
        assert!(
            xml.contains("New_DS"),
            "the configuration survived the script's sync: {xml}"
        );
    }
}
