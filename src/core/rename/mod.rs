//! Read-only planning for entity and dotted-prefix renames.
//!
//! A plan contains byte edits and filesystem moves but performs none of them. Planning refuses
//! invalid, nested, unknown, ambiguous or conflicting names, and refuses an incomplete workspace
//! discovery rather than presenting a partial rename as complete.

use super::baseline::{Baseline, RELATIVE_PATH as BASELINE_PATH};
use super::config::{Solution, CONFIG_FILE};
use super::ledger::{self, Ledger, LedgerError};
use super::{
    catalog, check, datashape, datatable, dbinfo, entity, refs, rename_property, rename_scan,
    rename_sql, repo, splice, types, workspace,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};

/// The identity scope being renamed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Entity,
    Prefix,
    Field,
    Service,
    Param,
    Table,
    Property,
}

/// A requested old-to-new rename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    pub kind: Kind,
    pub old: String,
    pub new: String,
    pub scope: Option<String>,
    /// Service containing the member, for a parameter rename.
    pub service: Option<String>,
}

/// One entity identity and its paths before and after an apply step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Move {
    pub collection: String,
    pub old_name: String,
    pub new_name: String,
    pub old_file: PathBuf,
    pub new_file: PathBuf,
    pub old_sidecars: Option<PathBuf>,
    pub new_sidecars: Option<PathBuf>,
    /// The committed FileRepository tree, when one exists for this entity name.
    pub old_repo_files: Option<PathBuf>,
    /// The destination of [`Move::old_repo_files`].
    pub new_repo_files: Option<PathBuf>,
}

/// The role a scanned file has in the solution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Entity,
    Sidecar,
    Config,
    Outside,
    Mashup,
}

/// Findings and applicable byte edits for one file as it exists at planning time.
#[derive(Debug)]
pub struct FileChange {
    pub path: PathBuf,
    pub kind: FileKind,
    /// SHA-256 of the bytes whose offsets the edits refer to.
    pub digest: [u8; 32],
    pub edits: Vec<splice::Edit>,
    pub findings: Vec<rename_scan::Finding>,
}

impl Kind {
    pub const ALL: [Kind; 7] = [
        Kind::Entity,
        Kind::Prefix,
        Kind::Field,
        Kind::Service,
        Kind::Param,
        Kind::Table,
        Kind::Property,
    ];

    /// The word that names the kind on the command line, in the tool's `kind` and in the ledger.
    pub fn word(self) -> &'static str {
        match self {
            Kind::Entity => "entity",
            Kind::Prefix => "prefix",
            Kind::Field => "field",
            Kind::Service => "service",
            Kind::Param => "param",
            Kind::Table => "table",
            Kind::Property => "property",
        }
    }

    pub fn from_word(word: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.word() == word)
    }

    /// What the kind is called in a sentence.
    pub fn noun(self) -> &'static str {
        if self == Kind::Param {
            "parameter"
        } else {
            self.word()
        }
    }

    /// A member rename names the entity that owns the member (its scope).
    pub fn is_member(self) -> bool {
        !matches!(self, Kind::Entity | Kind::Prefix)
    }

    /// Only entity, prefix and field renames can touch a database column or table.
    pub fn may_touch_database(self) -> bool {
        matches!(self, Kind::Entity | Kind::Prefix | Kind::Field)
    }

    /// The positional arguments after the kind, as usage text.
    pub fn positionals(self) -> &'static str {
        match self {
            Kind::Entity | Kind::Prefix => "<old> <new>",
            Kind::Param => "<entity> <service> <old> <new>",
            _ => "<entity> <old> <new>",
        }
    }

    /// Kind words as a sentence fragment (`a`, `b` or `c`), for a refusal.
    pub fn list_words() -> String {
        Kind::list_words_of(Kind::ALL.into_iter())
    }

    fn list_words_of(kinds: impl Iterator<Item = Kind>) -> String {
        let words: Vec<String> = kinds.map(|kind| format!("`{}`", kind.word())).collect();
        format!(
            "{} or {}",
            words[..words.len() - 1].join(", "),
            words[words.len() - 1]
        )
    }
}

/// How the caller chose to deal with the database half of a rename, as the front ends state it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DatabaseFlags {
    pub sql: bool,
    pub no_sql: bool,
    /// Where the migration script goes, relative to the solution; implies `sql`.
    pub dir: Option<String>,
}

/// What a front end (the command line, the MCP tool) asks for, before it is checked. One place
/// decides which combinations are valid, so the two cannot drift apart.
#[derive(Debug, Clone)]
pub struct Request {
    pub kind: Kind,
    pub scope: Option<String>,
    pub service: Option<String>,
    pub old: String,
    pub new: String,
    pub apply: bool,
    pub include_outside: bool,
    pub skip_checks: bool,
    pub database: DatabaseFlags,
    pub expect_digest: Option<String>,
}

impl Request {
    /// A request from positional names: `<old> <new>`, `<entity> <old> <new>` or, for a parameter,
    /// `<entity> <service> <old> <new>`. Flags are filled in by the caller.
    pub fn from_names(kind: Kind, names: &[String]) -> Result<Request, String> {
        let wanted = match kind {
            Kind::Entity | Kind::Prefix => 2,
            Kind::Param => 4,
            _ => 3,
        };
        if names.len() != wanted {
            return Err(format!(
                "rename {} needs {}",
                kind.word(),
                kind.positionals()
            ));
        }
        let (scope, service, rest) = match kind {
            Kind::Entity | Kind::Prefix => (None, None, names),
            Kind::Param => (Some(names[0].clone()), Some(names[1].clone()), &names[2..]),
            _ => (Some(names[0].clone()), None, &names[1..]),
        };
        Ok(Request {
            kind,
            scope,
            service,
            old: rest[0].clone(),
            new: rest[1].clone(),
            apply: false,
            include_outside: false,
            skip_checks: false,
            database: DatabaseFlags::default(),
            expect_digest: None,
        })
    }

    /// Check the combination and turn it into what `run` takes. `root` anchors the migration folder.
    pub fn build(self, root: &Path, date: &str) -> Result<(Spec, RunOptions), String> {
        let kind = self.kind;
        if kind.is_member() && self.scope.is_none() {
            return Err(format!(
                "scope is required when rename kind is `{}`",
                kind.word()
            ));
        }
        if !kind.is_member() && self.scope.is_some() {
            return Err(format!(
                "scope is only accepted for a {} rename",
                Kind::list_words_of(Kind::ALL.into_iter().filter(|kind| kind.is_member()))
            ));
        }
        if kind == Kind::Param && self.service.is_none() {
            return Err("service is required when rename kind is `param`".to_string());
        }
        if kind != Kind::Param && self.service.is_some() {
            return Err("service is only accepted when rename kind is `param`".to_string());
        }
        if self.include_outside && kind.is_member() {
            return Err(format!("a {} rename has no text pass", kind.noun()));
        }
        let database = &self.database;
        let wants_sql = database.sql || database.dir.is_some();
        if database.no_sql && wants_sql {
            return Err("no-sql and sql say different things; pass one or the other".to_string());
        }
        if (database.no_sql || wants_sql) && !kind.may_touch_database() {
            return Err(format!(
                "a {} rename has no database script; sql, sql-dir and no-sql are for entity, prefix and field renames",
                kind.noun()
            ));
        }
        let sql = if database.no_sql {
            SqlChoice::Off
        } else if wants_sql {
            SqlChoice::Write(root.join(database.dir.as_deref().unwrap_or("sql")))
        } else {
            SqlChoice::Unset
        };
        Ok((
            Spec {
                kind,
                old: self.old,
                new: self.new,
                scope: self.scope,
                service: self.service,
            },
            RunOptions {
                apply: self.apply,
                include_outside: self.include_outside,
                skip_checks: self.skip_checks,
                date: date.to_string(),
                sql,
                expect_digest: self.expect_digest,
            },
        ))
    }
}

/// A complete, read-only description of an entity or prefix rename.
#[derive(Debug)]
pub struct Plan {
    pub spec: Spec,
    pub moves: Vec<Move>,
    pub changes: Vec<FileChange>,
    pub outside: Vec<FileChange>,
    pub baseline_keys: Vec<(String, String)>,
    /// Paths named for the old identity which are reported but never renamed.
    pub named: Vec<PathBuf>,
    /// Non-UTF-8 sidecars and outside files, and outside files over 5 MiB.
    pub skipped: Vec<PathBuf>,
    /// Matching configuration-table instances, used by the field summary.
    pub field_tables: usize,
    /// Mashup files containing service rename findings.
    pub service_mashups: usize,
}

/// Controls which planned text is applied and supplies the caller's ledger date.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyOptions {
    /// Also change files outside source and entity folders.
    pub include_outside: bool,
    /// Recorded verbatim in the rename ledger.
    pub date: String,
    /// New files the rename writes beside its edits (the database migration), created only if absent
    /// and removed again on a rollback.
    pub extra_files: Vec<(PathBuf, Vec<u8>)>,
}

/// The completed workspace changes from applying a rename plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub files_changed: usize,
    pub moved: Vec<(PathBuf, PathBuf)>,
    pub baseline_removed: usize,
    pub ledger: PathBuf,
}

/// Controls gate preflight, optional mutation and the rename ledger date.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOptions {
    pub apply: bool,
    pub include_outside: bool,
    pub skip_checks: bool,
    pub date: String,
    /// What to do about the database half of a rename that touches DBConnection tables.
    pub sql: SqlChoice,
    /// The `plan_digest` of the plan the caller reviewed: if the workspace has changed since, the
    /// plan differs and the rename is refused before anything is written.
    pub expect_digest: Option<String>,
}

/// The caller's decision about the database half of a rename. There is no default that skips it:
/// a rename that touches a DBConnection table and was not told what to do about the data is refused.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SqlChoice {
    /// Nothing was said: refuse a rename that affects DBConnection tables.
    #[default]
    Unset,
    /// The tables are not in use (`--no-sql`): write no script.
    Off,
    /// Write the migration script into this folder.
    Write(PathBuf),
}

/// The migration script a rename produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlScript {
    pub path: PathBuf,
    pub text: String,
}

/// Post-apply checks. Findings are reported without rolling back a completed rename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verification {
    pub sync_problems: Vec<String>,
    pub blocking_gates: Vec<String>,
}

/// A rename plan and, when requested, its mutation and verification results.
#[derive(Debug)]
pub struct Outcome {
    pub plan: Plan,
    pub applied: Option<Applied>,
    pub verification: Option<Verification>,
    pub follow_up: Vec<String>,
    /// The database migration, when the rename needed one and the caller asked for it.
    pub sql: Option<SqlScript>,
}

/// One failure-injection boundary in the mutating portion of apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Step {
    Write(PathBuf),
    Move(PathBuf, PathBuf),
    Baseline,
    Ledger,
}

/// Finding totals for one file class.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct KindCounts {
    pub files: usize,
    pub exact: usize,
    pub embedded: usize,
    pub review: usize,
}

/// Finding totals grouped by the four planner file classes.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    pub entity: KindCounts,
    pub sidecar: KindCounts,
    pub config: KindCounts,
    pub outside: KindCounts,
    pub mashup: KindCounts,
}

/// A condition that prevents a complete and collision-free plan.
#[derive(Debug)]
pub enum RenameError {
    /// The rename affects DBConnection tables and nobody said what to do about the data.
    DatabaseHalf {
        shapes: Vec<String>,
        unsure: Vec<String>,
    },
    GatesFail {
        gates: Vec<String>,
    },
    InvalidNew {
        message: String,
    },
    InvalidOld {
        message: String,
    },
    Same {
        name: String,
    },
    Nested {
        old: String,
        new: String,
    },
    Unknown {
        old: String,
    },
    ServiceScope {
        message: String,
    },
    TableScope {
        message: String,
    },
    Ambiguous {
        old: String,
        files: Vec<PathBuf>,
    },
    Exists {
        conflicts: Vec<String>,
    },
    Unreadable {
        files: Vec<String>,
    },
    Io {
        path: PathBuf,
        why: String,
    },
    Xml {
        path: PathBuf,
        why: String,
    },
    Stale {
        path: PathBuf,
    },
    /// Applied to a scratch copy, the rename left these entities out of step with their sidecars.
    /// The plan is not the one the caller reviewed.
    PlanChanged {
        expected: String,
        actual: String,
    },
    WouldDesync {
        entities: Vec<String>,
    },
    InvalidLedger {
        path: PathBuf,
        why: String,
    },
    Apply {
        path: PathBuf,
        why: String,
    },
    Splice {
        path: PathBuf,
        why: String,
    },
    RollbackFailed {
        original: Box<RenameError>,
        leftover: Vec<PathBuf>,
    },
}

impl fmt::Display for RenameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RenameError::DatabaseHalf { shapes, unsure } => {
                write!(
                    f,
                    "this rename touches the database: {}. Pass --sql to write the migration script (run it before the import), or --no-sql if the tables are not in use",
                    if !shapes.is_empty() {
                        format!("DBConnection table(s) of {}", shapes.join(", "))
                    } else if !unsure.is_empty() {
                        "a GetDBInfo it could not read completely".to_string()
                    } else {
                        // An entity or prefix rename changes names that DBConnection rows store.
                        "this solution has DBConnection tables, whose rows store entity names".to_string()
                    }
                )?;
                if !unsure.is_empty() {
                    write!(f, " (could not read completely: {})", unsure.join(", "))?;
                }
                Ok(())
            }
            RenameError::GatesFail { gates } => write!(
                f,
                "rename preflight gates fail: {}; fix them or pass --skip-checks",
                gates.join(", ")
            ),
            RenameError::InvalidNew { message } => write!(f, "{message}; choose a valid new name"),
            RenameError::InvalidOld { message } => write!(f, "{message}; give the existing full name"),
            RenameError::Same { name } => write!(f, "old and new are both {name:?}; choose a different new name"),
            RenameError::Nested { old, new } => write!(
                f,
                "cannot rename nested prefixes {old:?} and {new:?} in one step; use an unrelated temporary prefix first"
            ),
            RenameError::Unknown { old } => write!(f, "no entity matches {old:?}; check the name and rename kind"),
            RenameError::ServiceScope { message } => f.write_str(message),
            RenameError::TableScope { message } => f.write_str(message),
            RenameError::Ambiguous { old, files } => write!(
                f,
                "entity {old:?} is ambiguous in {}; rename by prefix or remove the duplicate",
                files.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
            ),
            RenameError::Exists { conflicts } => write!(
                f,
                "rename targets already exist: {}; choose a new unused name or move them aside",
                conflicts.join(", ")
            ),
            RenameError::Unreadable { files } => write!(
                f,
                "cannot plan a complete rename because discovery could not read: {}; fix these files and retry",
                files.join(", ")
            ),
            RenameError::Io { path, why } => write!(
                f,
                "cannot read {} while planning the rename: {why}; fix its permissions and retry",
                path.display()
            ),
            RenameError::Xml { path, why } => write!(
                f,
                "cannot scan {} while planning the rename: {why}; repair the XML and retry",
                path.display()
            ),
            RenameError::Stale { path } => write!(
                f,
                "{} changed since the plan; run the rename again",
                path.display()
            ),
            RenameError::PlanChanged { expected, actual } => write!(
                f,
                "the plan changed since it was reviewed (reviewed {expected}, now {actual}); plan again and review it before applying"
            ),
            RenameError::WouldDesync { entities } => write!(
                f,
                "applied to a copy of the workspace, this rename leaves {} out of step with its sidecars; nothing was written (a rename bug: please report it)",
                entities.join(", ")
            ),
            RenameError::InvalidLedger { path, why } => write!(
                f,
                "cannot read rename ledger {}: {why}; repair or remove it and retry",
                path.display()
            ),
            RenameError::Apply { path, why } => write!(
                f,
                "cannot apply rename at {}: {why}; fix the filesystem problem and retry",
                path.display()
            ),
            RenameError::Splice { path, why } => write!(
                f,
                "cannot apply planned edits to {}: {why}; run the rename again",
                path.display()
            ),
            RenameError::RollbackFailed { original, leftover } => write!(
                f,
                "rename failed ({original}) and rollback left paths requiring repair: {}",
                leftover.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join(", ")
            ),
        }
    }
}

impl std::error::Error for RenameError {}

mod apply;
mod common;
mod field;
mod identity;
mod member;
mod run;
mod table;
#[cfg(test)]
mod tests;

pub use self::apply::apply;
pub use self::identity::plan;
pub use self::run::{db_effects, follow_up, run, DbEffects};
#[allow(unused_imports)]
use self::{apply::*, common::*, field::*, identity::*, member::*, run::*, table::*};

impl Plan {
    /// A fingerprint of everything the plan would do: the moves and, per file, the digest of the
    /// bytes it reads and every edit. The same workspace and request give the same digest, so a
    /// caller that applies by digest applies exactly the plan it was shown.
    pub fn digest(&self) -> String {
        let mut hasher = Sha256::new();
        let mut put = |bytes: &[u8]| {
            hasher.update((bytes.len() as u64).to_le_bytes());
            hasher.update(bytes);
        };
        put(self.spec.kind.word().as_bytes());
        for part in [&self.spec.old, &self.spec.new] {
            put(part.as_bytes());
        }
        for part in [&self.spec.scope, &self.spec.service] {
            put(part.as_deref().unwrap_or_default().as_bytes());
        }
        for item in &self.moves {
            for path in [&item.old_file, &item.new_file] {
                put(path.to_string_lossy().as_bytes());
            }
            for path in [
                &item.old_sidecars,
                &item.new_sidecars,
                &item.old_repo_files,
                &item.new_repo_files,
            ] {
                put(path
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_default()
                    .as_bytes());
            }
        }
        for change in self.changes.iter().chain(&self.outside) {
            put(change.path.to_string_lossy().as_bytes());
            put(&change.digest);
            for edit in &change.edits {
                put(&(edit.span.start as u64).to_le_bytes());
                put(&(edit.span.end as u64).to_le_bytes());
                put(&edit.replacement);
            }
        }
        let digest: [u8; 32] = hasher.finalize().into();
        digest.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// The line that names the rename, without its state.
    pub fn headline(&self) -> String {
        let spec = &self.spec;
        let scope = spec.scope.as_deref().unwrap_or_default();
        match spec.kind {
            Kind::Param => format!(
                "rename param {scope}.{}: {} -> {}",
                spec.service.as_deref().unwrap_or_default(),
                spec.old,
                spec.new
            ),
            kind if kind.is_member() => format!(
                "rename {} {scope}: {} -> {}",
                kind.word(),
                spec.old,
                spec.new
            ),
            kind => format!("rename {} {} -> {}", kind.word(), spec.old, spec.new),
        }
    }

    /// What the plan touches, as `(label, text)` rows, the same for every front end: the entities
    /// that move (identity renames) and the files and references changed, by place.
    pub fn summary_rows(&self, include_outside: bool, detail: bool) -> Vec<(&'static str, String)> {
        let counts = self.counts();
        let spread = |count: KindCounts, note: Option<&str>| {
            let references = count.exact + count.embedded;
            format!(
                "{} file{}, {references} reference{}{}",
                count.files,
                if count.files == 1 { "" } else { "s" },
                if references == 1 { "" } else { "s" },
                note.map(|text| format!(": {text}")).unwrap_or_default()
            )
        };
        let mut rows = Vec::new();
        let kind = self.spec.kind;
        if !kind.is_member() {
            let shown = if detail {
                self.moves.len()
            } else {
                self.moves.len().min(3)
            };
            let first = self
                .moves
                .iter()
                .take(shown)
                .map(|item| format!("{}/{} -> {}", item.collection, item.old_name, item.new_name))
                .collect::<Vec<_>>()
                .join(", ");
            let more = if shown < self.moves.len() {
                ", ..."
            } else {
                ""
            };
            rows.push((
                "moves",
                format!(
                    "{} entit{}, with their sidecar folders and repository folders{}",
                    self.moves.len(),
                    if self.moves.len() == 1 { "y" } else { "ies" },
                    if first.is_empty() {
                        String::new()
                    } else {
                        format!("; first: {first}{more}")
                    }
                ),
            ));
        }
        let present = |count: KindCounts| {
            count.files != 0 || count.exact + count.embedded + count.review != 0
        };
        match kind {
            Kind::Field => {
                rows.push((
                    "entity files",
                    format!("{}, {} tables", counts.entity.files, self.field_tables),
                ));
                rows.push(("sidecars", counts.sidecar.files.to_string()));
            }
            Kind::Service | Kind::Param => {
                rows.push(("entity files", spread(counts.entity, None)));
                rows.push(("sidecars", spread(counts.sidecar, None)));
                rows.push(("mashups", spread(counts.mashup, None)));
                rows.push(("twaco.toml", spread(counts.config, None)));
            }
            Kind::Table | Kind::Property => {
                rows.push((
                    "entity files",
                    format!("{}, {} tables", counts.entity.files, self.field_tables),
                ));
                rows.push(("scripts", counts.sidecar.files.to_string()));
            }
            Kind::Entity | Kind::Prefix => {
                let outside_note = if include_outside {
                    "changed"
                } else {
                    "not changed unless --text"
                };
                for (label, count, note) in [
                    ("entity files", counts.entity, None),
                    ("sidecars", counts.sidecar, None),
                    ("twaco.toml", counts.config, None),
                    ("elsewhere", counts.outside, Some(outside_note)),
                ] {
                    if present(count) {
                        rows.push((label, spread(count, note)));
                    }
                }
            }
        }
        rows
    }

    /// Counts files and findings in each file class; every finding contributes exactly once.
    pub fn counts(&self) -> Counts {
        let mut counts = Counts::default();
        for change in self.changes.iter().chain(&self.outside) {
            let target = match change.kind {
                FileKind::Entity => &mut counts.entity,
                FileKind::Sidecar => &mut counts.sidecar,
                FileKind::Config => &mut counts.config,
                FileKind::Outside => &mut counts.outside,
                FileKind::Mashup => &mut counts.mashup,
            };
            target.files += 1;
            for finding in &change.findings {
                match finding.tier {
                    refs::Tier::Exact => target.exact += 1,
                    refs::Tier::Embedded => target.embedded += 1,
                    refs::Tier::Review => target.review += 1,
                }
            }
        }
        counts
    }
}

/// Builds the shared structured summary used by the CLI and MCP adapters.
///
/// `item_limit` bounds move, review and named-path examples without changing their total counts.
pub fn summary_json(
    solution: &Solution,
    outcome: &Outcome,
    include_outside: bool,
    item_limit: usize,
) -> serde_json::Value {
    let plan = &outcome.plan;
    let counts = plan.counts();
    let count = |value: KindCounts| {
        serde_json::json!({
            "files": value.files,
            "exact": value.exact,
            "embedded": value.embedded,
            "review": value.review,
        })
    };
    let relative = |path: &Path| {
        path.strip_prefix(&solution.root)
            .unwrap_or(path)
            .display()
            .to_string()
            .replace('\\', "/")
    };
    let review = plan
        .changes
        .iter()
        .chain(&plan.outside)
        .flat_map(|change| {
            change
                .findings
                .iter()
                .filter(|finding| finding.tier == refs::Tier::Review)
                .map(|finding| {
                    serde_json::json!({
                        "file": relative(&change.path),
                        "line": finding.line,
                        "excerpt": finding.excerpt,
                    })
                })
        })
        .take(item_limit)
        .collect::<Vec<_>>();
    let verification = outcome.verification.as_ref().map(|value| {
        serde_json::json!({
            "sync_problems": value.sync_problems,
            "blocking_gates": value.blocking_gates,
        })
    });
    serde_json::json!({
        "spec": {
            "kind": plan.spec.kind.word(),
            "old": plan.spec.old,
            "new": plan.spec.new,
            "scope": plan.spec.scope,
            "service": plan.spec.service,
        },
        "plan_digest": plan.digest(),
        "moves": {
            "count": plan.moves.len(),
            "first": plan.moves.iter().take(item_limit).map(|item| serde_json::json!({
                "collection": item.collection, "old": item.old_name, "new": item.new_name,
            })).collect::<Vec<_>>(),
        },
        "tables": plan.field_tables,
        "counts": {
            "entity": count(counts.entity),
            "sidecar": count(counts.sidecar),
            "config": count(counts.config),
            "outside": count(counts.outside),
            "mashup": count(counts.mashup),
        },
        "review": review,
        "named": {
            "count": plan.named.len(),
            "first": plan.named.iter().take(item_limit).map(|path| relative(path)).collect::<Vec<_>>(),
        },
        "outside_applied": outcome.applied.is_some() && include_outside,
        "applied": outcome.applied.is_some(),
        "verification": verification,
        "follow_up": outcome.follow_up,
        "sql": outcome.sql.as_ref().map(|script| {
            let relative = script.path.strip_prefix(&solution.root).unwrap_or(&script.path);
            serde_json::json!({
                "path": relative.display().to_string().replace(std::path::MAIN_SEPARATOR, "/"),
                "bytes": script.text.len(),
                "written": outcome.applied.is_some(),
            })
        }),
        "skipped": plan.skipped.len(),
    })
}
