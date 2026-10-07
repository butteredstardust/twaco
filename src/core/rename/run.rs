//! Running a rename: gates, plan, database half, verification in a scratch copy, apply, verification.

use super::*;

/// Plans a rename, optionally applies it, and verifies the resulting workspace. Applying needs
/// the workspace lock, which the caller holds for the whole command.
pub fn run(
    solution: &Solution,
    spec: &Spec,
    options: &RunOptions,
    lock: Option<&WorkspaceLock>,
) -> Result<Outcome, RenameError> {
    run_with(solution, spec, options, lock, &mut |_| {})
}

/// `run`, with a hook that may change the scratch copy the rename is verified in (tests only).
pub(crate) fn run_with(
    solution: &Solution,
    spec: &Spec,
    options: &RunOptions,
    lock: Option<&WorkspaceLock>,
    scratch_hook: &mut dyn FnMut(&Solution),
) -> Result<Outcome, RenameError> {
    if !options.skip_checks {
        let report = check::run(solution);
        if report.blocks() {
            return Err(RenameError::GatesFail {
                gates: report
                    .gates
                    .iter()
                    .filter(|gate| gate.blocks())
                    .map(|gate| gate.name.clone())
                    .collect(),
            });
        }
    }
    let plan = plan(solution, spec)?;
    if let Some(expected) = &options.expect_digest {
        let actual = plan.digest();
        if &actual != expected {
            return Err(RenameError::PlanChanged {
                expected: expected.clone(),
                actual,
            });
        }
    }
    // The database half is decided before anything is written: a rename that touches DBConnection
    // tables is refused unless the caller said what to do about their data.
    let effects = db_effects(solution, &plan, &options.date);
    let mut sql = None;
    if effects.needs_decision() {
        match &options.sql {
            SqlChoice::Unset => {
                return Err(RenameError::DatabaseHalf {
                    shapes: effects.shapes.clone(),
                    unsure: effects.unsure.clone(),
                })
            }
            SqlChoice::Off => {}
            SqlChoice::Write(folder) if !effects.migration.is_empty() => {
                let name = rename_sql::file_name(&options.date, &effects.label);
                sql = Some(SqlScript {
                    path: folder.join(name),
                    text: rename_sql::render(&effects.migration),
                });
            }
            SqlChoice::Write(_) => {}
        }
    }
    let mut notes = Vec::new();
    if options.apply {
        match verify_before_writing(solution, spec, &plan, options, scratch_hook)? {
            Verified::Clean => {}
            Verified::Skipped(why) => notes.push(format!(
                "The rename was not verified before writing: {why}."
            )),
        }
    }
    let (applied, verification) = if options.apply {
        let lock = lock.ok_or_else(|| RenameError::Apply {
            path: solution.root.clone(),
            why: "applying needs the workspace lock".to_string(),
        })?;
        let applied = apply(
            solution,
            &plan,
            &ApplyOptions {
                include_outside: options.include_outside,
                date: options.date.clone(),
                extra_files: sql
                    .iter()
                    .map(|script| (script.path.clone(), script.text.clone().into_bytes()))
                    .collect(),
            },
            lock,
        )?;
        // A prefix rename may change project declarations in twaco.toml. Verification must use
        // those new declarations rather than the in-memory solution used to make the plan.
        let verified_solution =
            Solution::load(&solution.root.join(CONFIG_FILE)).map_err(|error| {
                RenameError::Apply {
                    path: solution.root.join(CONFIG_FILE),
                    why: error.to_string(),
                }
            })?;
        let sync_problems = sync_problems(&verified_solution);
        let report = check::run(&verified_solution);
        let blocking_gates = report
            .gates
            .iter()
            .filter(|gate| gate.blocks())
            .map(|gate| gate.name.clone())
            .collect();
        (
            Some(applied),
            Some(Verification {
                sync_problems,
                blocking_gates,
            }),
        )
    } else {
        (None, None)
    };
    let mut follow_up = follow_up(&plan, options.include_outside);
    follow_up.extend(notes);
    if let Some(script) = &sql {
        follow_up.push(format!(
            "Run {} before the import: `twaco db run {} --apply` (one atomic script), or psql with ON_ERROR_STOP=1. DeployComponent creates tables and never renames one.",
            script.path.display(),
            script.path.display()
        ));
    }
    for note in &effects.migration.notes {
        follow_up.push(note.clone());
    }
    Ok(Outcome {
        plan,
        applied,
        verification,
        follow_up,
        sql,
    })
}

/// The entities whose sidecars and entity document are out of step, as `Collection/Name`, plus any
/// file discovery could not read.
pub(super) fn sync_problems(solution: &Solution) -> Vec<String> {
    let discovered = workspace::discover(solution);
    let mut problems = Vec::new();
    for entity in &discovered.entities {
        let outcome = crate::core::workflow::sync(
            solution,
            std::slice::from_ref(entity),
            &[],
            crate::core::workflow::SyncOptions {
                check: true,
                ..Default::default()
            },
        );
        if outcome.changed > 0 || outcome.failed > 0 {
            problems.push(format!("{}/{}", entity.info.collection, entity.info.name));
        }
    }
    problems.extend(discovered.unreadable);
    problems
}

/// Folders that are never part of a workspace's sources, and so are not copied for verification.
const SCRATCH_SKIPS: [&str; 4] = [".git", ".twaco", "node_modules", "target"];
/// A workspace larger than this is not verified in a copy.
const SCRATCH_LIMIT_BYTES: u64 = 1 << 30;

enum Verified {
    Clean,
    /// Why the check could not be made; the rename goes on without it.
    Skipped(String),
}

/// Copy the workspace; `false` when it is over the limit.
pub(super) fn copy_workspace(from: &Path, to: &Path, total: &mut u64) -> std::io::Result<bool> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        let kind = entry.file_type()?;
        if kind.is_dir() {
            if SCRATCH_SKIPS.contains(&name.to_string_lossy().as_ref()) {
                continue;
            }
            if !copy_workspace(&entry.path(), &to.join(&name), total)? {
                return Ok(false);
            }
        } else if kind.is_file() {
            *total += entry.metadata()?.len();
            if *total > SCRATCH_LIMIT_BYTES {
                return Ok(false);
            }
            std::fs::copy(entry.path(), to.join(&name))?;
        }
    }
    Ok(true)
}

/// Apply the rename to a throwaway copy of the workspace and refuse, before the real workspace is
/// touched, if that leaves an entity's sidecars out of step with its document when they were in
/// step before. The check after the real apply stays as a second line.
fn verify_before_writing(
    solution: &Solution,
    spec: &Spec,
    plan: &Plan,
    options: &RunOptions,
    scratch_hook: &mut dyn FnMut(&Solution),
) -> Result<Verified, RenameError> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let scratch = std::env::temp_dir().join(format!(
        "twaco-verify-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let result = (|| {
        let mut total = 0;
        match copy_workspace(&solution.root, &scratch, &mut total) {
            Ok(true) => {}
            Ok(false) => {
                return Ok(Verified::Skipped(
                    "the workspace is too large to copy".to_string(),
                ))
            }
            Err(error) => {
                return Ok(Verified::Skipped(format!(
                    "the workspace could not be copied: {error}"
                )))
            }
        }
        let Ok(copy) = Solution::load(&scratch.join(CONFIG_FILE)) else {
            return Ok(Verified::Skipped(
                "the copy's configuration did not load".to_string(),
            ));
        };
        let before = sync_problems(&copy);
        let Ok(copy_plan) = self::plan(&copy, spec) else {
            return Ok(Verified::Skipped(
                "the copy could not be planned".to_string(),
            ));
        };
        if (copy_plan.moves.len(), copy_plan.changes.len())
            != (plan.moves.len(), plan.changes.len())
        {
            return Ok(Verified::Skipped(
                "the copy does not hold everything the plan touches (a source folder outside the workspace?)".to_string(),
            ));
        }
        let apply_options = ApplyOptions {
            include_outside: options.include_outside,
            date: options.date.clone(),
            extra_files: Vec::new(),
        };
        // The copy is a workspace of its own, with its own lock.
        let copy_lock =
            crate::core::lock::acquire(&scratch, "rename verification", &[]).map_err(|error| {
                RenameError::Apply {
                    path: scratch.clone(),
                    why: error.to_string(),
                }
            })?;
        apply(&copy, &copy_plan, &apply_options, &copy_lock)?;
        drop(copy_lock);
        scratch_hook(&copy);
        let after_solution =
            Solution::load(&scratch.join(CONFIG_FILE)).map_err(|error| RenameError::Apply {
                path: scratch.join(CONFIG_FILE),
                why: error.to_string(),
            })?;
        // A renamed entity was in step (or not) under its old name.
        let old_name = |problem: &str| -> String {
            plan.moves
                .iter()
                .find(|item| format!("{}/{}", item.collection, item.new_name) == problem)
                .map_or_else(
                    || problem.to_string(),
                    |item| format!("{}/{}", item.collection, item.old_name),
                )
        };
        let introduced: Vec<String> = sync_problems(&after_solution)
            .into_iter()
            .filter(|problem| !before.contains(&old_name(problem)))
            .collect();
        if introduced.is_empty() {
            Ok(Verified::Clean)
        } else {
            Err(RenameError::WouldDesync {
                entities: introduced,
            })
        }
    })();
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

/// What a rename does to DBConnection tables: the script it would need, and why a decision is wanted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DbEffects {
    /// Backed DataShapes the rename touches (full old names).
    pub shapes: Vec<String>,
    pub migration: rename_sql::Migration,
    /// `GetDBInfo` scripts that could not be read completely.
    pub unsure: Vec<String>,
    /// Part of the script's file name.
    pub label: String,
    affected: bool,
}

impl DbEffects {
    /// Whether the caller must say what to do about the database.
    pub fn needs_decision(&self) -> bool {
        self.affected
    }
}

/// Compute the database half of a rename from the solution's `GetDBInfo` scripts. Only the
/// DataShapes those scripts declare are DBConnection tables; a service, a parameter and a
/// configuration table never are.
pub fn db_effects(solution: &Solution, plan: &Plan, date: &str) -> DbEffects {
    let info = dbinfo::load(solution);
    let mut effects = DbEffects {
        unsure: info.unsure.clone(),
        ..Default::default()
    };
    if info.shapes.is_empty() && info.unsure.is_empty() {
        return effects;
    }
    let spec = &plan.spec;
    let short = |name: &str| name.rsplit('.').next().unwrap_or(name).to_string();
    let columns_of = |indexes: &[Vec<dbinfo::Name>]| -> Vec<Vec<String>> {
        indexes
            .iter()
            .map(|index| {
                index
                    .iter()
                    .map(|name| dbinfo::column_of(&name.value))
                    .collect()
            })
            .collect()
    };
    let foreign_columns = |shape: &dbinfo::DbShape| -> Vec<String> {
        shape
            .foreign_keys
            .iter()
            .filter_map(|key| key.column.as_ref())
            .map(|name| dbinfo::column_of(&name.value))
            .collect()
    };
    effects.migration.date = date.to_string();
    match spec.kind {
        Kind::Field => {
            let scope = spec.scope.as_deref().unwrap_or_default();
            let Some(shape) = info.shapes.get(scope) else {
                return effects;
            };
            let (old, new) = (dbinfo::column_of(&spec.old), dbinfo::column_of(&spec.new));
            effects.shapes.push(scope.to_string());
            effects.affected = true;
            effects.label = format!("field-{}-{}-to-{}", short(scope), spec.old, spec.new);
            effects.migration.title =
                format!("Rename field {}.{} to {}", scope, spec.old, spec.new);
            if old != new {
                effects.migration.columns.push(rename_sql::ColumnRename {
                    table: dbinfo::table_of(scope),
                    old,
                    new,
                    indexes: columns_of(&shape.indexes),
                    foreign_keys: foreign_columns(shape),
                });
            }
        }
        Kind::Entity | Kind::Prefix => {
            let word = if spec.kind == Kind::Entity {
                "entity"
            } else {
                "prefix"
            };
            effects.affected = true;
            effects.label = format!("{word}-{}-to-{}", spec.old, spec.new);
            effects.migration.title = format!("Rename {word} {} to {}", spec.old, spec.new);
            let mut renamed: std::collections::BTreeMap<String, String> = Default::default();
            for item in plan
                .moves
                .iter()
                .filter(|item| item.collection == "DataShapes")
            {
                let Some(shape) = info.shapes.get(&item.old_name) else {
                    continue;
                };
                effects.shapes.push(item.old_name.clone());
                let (old_table, new_table) = (
                    dbinfo::table_of(&item.old_name),
                    dbinfo::table_of(&item.new_name),
                );
                if old_table != new_table {
                    effects.migration.tables.push(rename_sql::TableRename {
                        old_table: old_table.clone(),
                        new_table: new_table.clone(),
                        indexes: columns_of(&shape.indexes),
                        foreign_keys: foreign_columns(shape),
                    });
                    renamed.insert(old_table, new_table);
                }
            }
            // Rows name entities by their full name, so the text of every DBConnection table changes.
            effects
                .migration
                .replacements
                .push((spec.old.clone(), spec.new.clone()));
            let mut tables: Vec<String> = info
                .shapes
                .keys()
                .map(|name| dbinfo::table_of(name))
                .map(|table| renamed.get(&table).cloned().unwrap_or(table))
                .collect();
            tables.sort();
            tables.dedup();
            effects.migration.backed_tables = tables;
        }
        Kind::Service | Kind::Param | Kind::Table | Kind::Property => {}
    }
    for origin in &info.unsure {
        effects.migration.notes.push(format!(
            "{origin} builds its table list at run time, so twaco could not read every table: check it by hand"
        ));
    }
    effects
}

/// Warnings for state and references that a repository rename cannot carry.
pub fn follow_up(plan: &Plan, outside_included: bool) -> Vec<String> {
    if plan.spec.kind == Kind::Param {
        let review = plan
            .changes
            .iter()
            .flat_map(|change| &change.findings)
            .filter(|finding| finding.tier == refs::Tier::Review)
            .count();
        return vec![
            "Callers outside this repository (REST clients, other projects and schedulers) pass parameters by name and break.".to_string(),
            format!("{review} occurrence(s) were left for a person to review."),
        ];
    }
    if plan.spec.kind == Kind::Property {
        let review = plan
            .changes
            .iter()
            .flat_map(|change| &change.findings)
            .filter(|finding| finding.tier == refs::Tier::Review)
            .count();
        return vec![
            "A deploy writes the renamed property from the entity XML; a value changed at run time is not carried, and the old property's value stays under the old name on a server.".to_string(),
            "A ValueStream's or Stream's logged data stays under the old property name.".to_string(),
            "Callers outside this repository (REST clients, other projects, mashups on another server) read the property by name and break.".to_string(),
            format!("{review} occurrence(s) were left for a person to review."),
        ];
    }
    if plan.spec.kind == Kind::Table {
        let review = plan
            .changes
            .iter()
            .flat_map(|change| &change.findings)
            .filter(|finding| finding.tier == refs::Tier::Review)
            .count();
        return vec![
            "The new table is a new table on a server: its rows come from the entity XML, so deploy with --overwrite-tables to load them; values edited on the server are not carried.".to_string(),
            "The table's DataShape keeps its name; rename it with twaco rename entity if it is named for the table.".to_string(),
            format!("{review} occurrence(s) were left for a person to review."),
            "Services that read or write the table by a name built at run time are not changed.".to_string(),
        ];
    }
    if plan.spec.kind == Kind::Service {
        let mut items = vec![
            "A deploy replaces the entity's services: the old name disappears from the server and anything outside this repository that calls it (other projects, REST clients, schedulers) breaks".to_string(),
        ];
        let review = plan
            .changes
            .iter()
            .flat_map(|change| &change.findings)
            .filter(|finding| finding.tier == refs::Tier::Review)
            .count();
        items.push(format!(
            "{review} occurrence(s) were left for a person to review."
        ));
        if let Some(scope) = plan.spec.scope.as_deref().filter(|scope| {
            plan.moves.iter().any(|item| {
                item.old_name == *scope
                    && matches!(item.collection.as_str(), "ThingShapes" | "ThingTemplates")
            })
        }) {
            let inherited: Vec<&str> = plan
                .moves
                .iter()
                .map(|item| item.old_name.as_str())
                .filter(|name| *name != scope)
                .collect();
            if !inherited.is_empty() {
                items.push(format!(
                    "Entities that inherit it were renamed too: {}",
                    inherited.join(", ")
                ));
            }
        }
        return items;
    }
    if plan.spec.kind == Kind::Field {
        return vec![
            "The rows of a configuration table keep their values on the server unless you deploy with --overwrite-tables".to_string(),
            format!("Scripts, mashup bindings and services that read the field by name are not changed; search for {:?}", plan.spec.old),
            "If this shape is stored through DBConnection, the column must be renamed in the database before the import".to_string(),
        ];
    }
    let mut items = vec![
        "The old entities stay on any server that has them (an import never removes one); `twaco deploy` creates the renamed ones. .twaco/renames.json lists the old names.".to_string(),
    ];
    if plan
        .moves
        .iter()
        .any(|item| matches!(item.collection.as_str(), "Things" | "DataTables"))
    {
        items.push("Persisted property values, a Stream's or ValueStream's logged data and DataTable rows stay with the old Thing, and a renamed DataTable starts empty.".to_string());
    }
    if plan.moves.iter().any(|item| item.old_repo_files.is_some()) {
        items.push("Files already on a server stay in the old repository, and deleting that repository deletes them; `twaco repo push` uploads the renamed folder.".to_string());
    }
    if plan.moves.iter().any(|item| {
        matches!(
            item.collection.as_str(),
            "Groups" | "Organizations" | "Users"
        )
    }) {
        items.push("Memberships are server state; add them again on the new entity.".to_string());
    }
    if plan.moves.iter().any(|item| item.collection == "Projects") {
        items.push("Another project that `dependsOn` the old one blocks deleting it.".to_string());
    }
    if !outside_included && !plan.outside.is_empty() {
        let references = plan
            .outside
            .iter()
            .flat_map(|change| &change.findings)
            .filter(|finding| finding.tier != refs::Tier::Review)
            .count();
        items.push(format!(
            "{references} reference(s) in {} other file(s) were not changed (docs, sql, localization ...); pass --text.",
            plan.outside.len()
        ));
    }
    let review = plan
        .changes
        .iter()
        .chain(&plan.outside)
        .flat_map(|change| &change.findings)
        .filter(|finding| finding.tier == refs::Tier::Review)
        .count();
    if review > 0 {
        items.push(format!(
            "{review} occurrence(s) were left for a person to review."
        ));
    }
    items
}
