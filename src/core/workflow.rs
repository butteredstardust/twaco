//! The local write loop, `sync`, as a result rather than as printing.
//!
//! The CLI prints the log in order; the MCP server returns it. Both run exactly this, so they
//! cannot disagree about what a sync did. The per-kind steps were moved here from the CLI
//! unchanged, printing turned into log lines.

use super::config::Solution;
use super::workspace::EntityFile;
use std::collections::BTreeMap;

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

    pub fn changes(&self) -> impl Iterator<Item = &str> {
        self.lines.iter().filter_map(|line| match line {
            Line::Change(text) => Some(text.as_str()),
            Line::Error(_) => None,
        })
    }

    pub fn errors(&self) -> impl Iterator<Item = &str> {
        self.lines.iter().filter_map(|line| match line {
            Line::Error(text) => Some(text.as_str()),
            Line::Change(_) => None,
        })
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SyncOptions {
    /// Report what would change and write nothing.
    pub check: bool,
    /// Permit a service or field to appear or disappear.
    pub allow_structural: bool,
    /// Rewrite every script payload in the configured layout.
    pub relayout: bool,
    /// The entities were asked for by name, so one with no sidecars is a failure rather than
    /// simply not under management.
    pub named: bool,
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
pub fn sync(
    solution: &Solution,
    chosen: &[EntityFile],
    unreadable: &[String],
    options: SyncOptions,
) -> SyncOutcome {
    let mut outcome = SyncOutcome::default();
    let log = &mut outcome.log;
    for problem in unreadable {
        log.error(problem.clone());
    }
    let mut failed = unreadable.len();
    for entity in chosen {
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
        if super::datatable::is_data_table(&src) {
            tally.record(sync_datatable(
                log,
                solution,
                entity,
                &mut current,
                options.check,
            ));
        }

        let dir = super::workspace::services_dir(solution, entity);
        let sidecars = super::workspace::read_sidecars(&dir);
        if !sidecars.is_empty() {
            tally.record(sync_services(
                log,
                entity,
                &mut current,
                &sidecars,
                options.check,
                options.allow_structural,
                solution.format.indent_cdata_payload,
                options.relayout,
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
        if !options.check && current != src {
            if let Err(e) = super::workspace::write_entity(&entity.path, &current) {
                log.error(format!("{e}"));
                tally.changed = false;
                tally.failed += 1;
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
    outcome.types =
        super::types::refresh_after_write(solution, !options.check && outcome.changed > 0);
    outcome
}

/// Fold one entity's service sidecars into `current`. Returns whether anything changed.
#[allow(clippy::too_many_arguments)]
fn sync_services(
    log: &mut Log,
    entity: &super::workspace::EntityFile,
    current: &mut Vec<u8>,
    sidecars: &BTreeMap<String, super::sidecar::ServiceSidecar>,
    check: bool,
    allow_structural: bool,
    indent_cdata_payload: bool,
    relayout: bool,
) -> Result<Option<bool>, ()> {
    match super::sync::sync(
        current,
        sidecars,
        allow_structural,
        indent_cdata_payload,
        relayout,
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
    let desired = match super::datashape::from_sidecar(&text.replace("\r\n", "\n")) {
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
pub fn extract(
    solution: &Solution,
    chosen: &[EntityFile],
    unreadable: &[String],
    named: bool,
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

        // A DataShape's fields are a sidecar of their own. Not `continue`: a DataShape may
        // also declare services, and returning early here would silently skip them.
        let mut did_something = false;
        if entity.info.collection == "DataShapes" {
            match super::datashape::extract(&src) {
                Ok(fields) => {
                    let path = super::workspace::fields_path(solution, entity);
                    let text = super::datashape::to_sidecar(&fields);
                    match super::workspace::write_fields(&path, &text) {
                        Ok(()) => {
                            entities += 1;
                            written += fields.len();
                            did_something = true;
                            log.change(format!(
                                "{}: {} field(s) -> {}",
                                entity.info.name,
                                fields.len(),
                                path.display()
                            ));
                        }
                        Err(e) => {
                            log.error(format!("{e}"));
                            failed += 1;
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
                    match super::workspace::write_mashup(&dir, &assets) {
                        Ok(()) => {
                            entities += 1;
                            written += 2;
                            did_something = true;
                            log.change(format!(
                                "{}: content and stylesheet -> {}",
                                entity.info.name,
                                dir.display()
                            ));
                        }
                        Err(e) => {
                            log.error(format!("{e}"));
                            failed += 1;
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
                    match super::workspace::write_datatable(&path, &text) {
                        Ok(()) => {
                            entities += 1;
                            written += 1;
                            did_something = true;
                            log.change(format!(
                                "{}: {} index(es) and a shape -> {}",
                                entity.info.name,
                                configuration.indexes.len(),
                                path.display()
                            ));
                        }
                        Err(e) => {
                            log.error(format!("{e}"));
                            failed += 1;
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
            Ok(extraction) => {
                if extraction.services.is_empty() {
                    // Naming an entity with nothing to extract is worth saying; sweeping past
                    // it under --all is not, and neither is one whose fields or mashup came out.
                    if named && !did_something {
                        log.change(format!("{}: no script services", entity.info.name));
                        report_skipped(log, &extraction);
                    }
                    continue;
                }
                let dir = super::workspace::services_dir(solution, entity);
                match super::workspace::write_sidecars(&dir, &extraction.services) {
                    Ok(stale) => {
                        entities += 1;
                        written += extraction.services.len();
                        log.change(format!(
                            "{}: {} service(s) -> {}",
                            entity.info.name,
                            extraction.services.len(),
                            dir.display()
                        ));
                        report_skipped(log, &extraction);
                        if !stale.is_empty() {
                            log.change(format!(
                                "    no longer in the entity: {}",
                                stale.join(", ")
                            ));
                        }
                    }
                    Err(e) => {
                        log.error(format!("{e}"));
                        failed += 1;
                    }
                }
            }
            Err(e) => {
                log.error(format!("{}: {e}", entity.path.display()));
                failed += 1;
            }
        }
    }
    outcome.written = written;
    outcome.entities = entities;
    outcome.failed = failed;
    outcome.types = super::types::refresh_after_write(solution, written > 0);
    outcome
}

/// Say what an entity held that did not become a sidecar, so it is never a silent omission.
fn report_skipped(log: &mut Log, extraction: &super::sidecar::Extraction) {
    if !extraction.non_script.is_empty() {
        log.change(format!(
            "    not scripts: {}",
            extraction.non_script.join(", ")
        ));
    }
    if !extraction.inherited.is_empty() {
        log.change(format!(
            "    defined elsewhere: {}",
            extraction.inherited.join(", ")
        ));
    }
    if !extraction.without_script.is_empty() {
        log.change(format!(
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
pub fn fmt(solution: &Solution, check: bool) -> FmtOutcome {
    let mut outcome = FmtOutcome::default();
    let style = super::fmt::Style::default();
    let files = super::workspace::script_files(solution);
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

    fn service_fixture(label: &str) -> (PathBuf, Solution) {
        let nonce = crate::test_nonce();
        let root = std::env::temp_dir().join(format!(
            "twaco-workflow-{label}-{}-{nonce}",
            std::process::id()
        ));
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
        (root, solution)
    }

    fn entities(solution: &Solution) -> Vec<EntityFile> {
        workspace::discover(solution).entities
    }

    #[test]
    fn sync_refreshes_only_after_a_real_write_and_only_when_opted_in() {
        let (root, solution) = service_fixture("sync-types");
        types::write(&solution).unwrap();
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

        let written = sync(&solution, &entities(&solution), &[], SyncOptions::default());
        assert_eq!(written.changed, 1);
        assert!(written.types.files_written.is_some());
        assert!(std::fs::read_to_string(&globals)
            .unwrap()
            .contains("declare let added: string;"));

        std::fs::remove_file(&globals).unwrap();
        let unchanged = sync(&solution, &entities(&solution), &[], SyncOptions::default());
        assert_eq!(unchanged.changed, 0);
        assert!(unchanged.types.files_written.is_none());
        assert!(!globals.exists());
        let _ = std::fs::remove_dir_all(root);

        let (root, solution) = service_fixture("sync-no-types");
        std::fs::write(root.join("src/T/services/Run/script.js"), "changed();").unwrap();
        let outcome = sync(&solution, &entities(&solution), &[], SyncOptions::default());
        assert_eq!(outcome.changed, 1);
        assert!(!root.join(".twaco/types").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn every_kind_of_sidecar_an_entity_has_lands_in_its_one_write() {
        let nonce = crate::test_nonce();
        let root = std::env::temp_dir().join(format!(
            "twaco-workflow-kinds-{}-{nonce}",
            std::process::id()
        ));
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
        let extracted = extract(&solution, &entities(&solution), &[], false);
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
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn extract_adds_the_service_type_project_and_refresh_failure_is_advisory() {
        let (root, solution) = service_fixture("extract-types");
        std::fs::remove_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join(".twaco/types")).unwrap();
        let outcome = extract(&solution, &entities(&solution), &[], false);
        assert!(outcome.written > 0);
        assert!(root.join("src/T/services/Run/jsconfig.json").is_file());
        assert!(outcome.types.warning.is_none());
        let _ = std::fs::remove_dir_all(root);

        let (root, solution) = service_fixture("extract-no-types");
        std::fs::remove_dir_all(root.join("src")).unwrap();
        let outcome = extract(&solution, &entities(&solution), &[], false);
        assert!(outcome.written > 0);
        assert!(outcome.types.files_written.is_none());
        assert!(!root.join(".twaco/types").exists());
        assert!(!root.join("src/T/services/Run/jsconfig.json").exists());
        let _ = std::fs::remove_dir_all(root);

        let (root, solution) = service_fixture("refresh-warning");
        std::fs::create_dir_all(root.join(".twaco")).unwrap();
        std::fs::write(root.join(".twaco/types"), "not a directory").unwrap();
        std::fs::write(root.join("src/T/services/Run/script.js"), "changed();").unwrap();
        let outcome = sync(&solution, &entities(&solution), &[], SyncOptions::default());
        assert_eq!(outcome.changed, 1);
        assert_eq!(outcome.failed, 0);
        assert!(outcome.types.warning.is_some());
        assert!(std::fs::read_to_string(root.join("Things/T.xml"))
            .unwrap()
            .contains("changed();"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_data_table_keeps_its_configuration_when_a_service_syncs_after_it() {
        // Two kinds of sidecar write into the one entity file. The second must start from what
        // the first wrote, not from the file as it was before either.
        let nonce = crate::test_nonce();
        let root =
            std::env::temp_dir().join(format!("twaco-workflow-dt-{}-{nonce}", std::process::id()));
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
        let extracted = extract(&solution, &entities(&solution), &[], false);
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
        let _ = std::fs::remove_dir_all(root);
    }
}
