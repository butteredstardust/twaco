//! Domain operations for localization tables.  They plan every replacement before writing it.

use super::{
    compare, discover, edit, file_name, prefixes, problems, render, root, target, Compared, Edit,
    Header, LocalizationError, Problem, Remote, State, TableFile, Token, DEFAULT_TABLE,
};
use crate::core::config::{Project, Solution};
use crate::core::imports;
use crate::core::parallel;
use crate::core::progress::{self, Progress};
use crate::core::workspace;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    pub path: PathBuf,
    pub created: bool,
    pub set: Vec<String>,
    pub removed: Vec<String>,
}

#[derive(Debug)]
pub struct Status {
    pub compared: Vec<Compared>,
    pub problems: Vec<Problem>,
    pub missing_tables: Vec<String>,
    pub unreadable: Vec<(PathBuf, String)>,
}

#[derive(Debug)]
pub struct Pulled {
    pub files: Vec<FileChange>,
    pub applied: bool,
}

#[derive(Debug)]
pub struct TablePush {
    pub table: String,
    pub files: Vec<PathBuf>,
    pub create: bool,
    pub set: Vec<String>,
    pub delete: Vec<String>,
}

#[derive(Debug)]
pub struct Pushed {
    pub tables: Vec<TablePush>,
    pub applied: bool,
}

#[derive(Debug)]
pub struct Edited {
    pub files: Vec<FileChange>,
    pub applied: bool,
}

fn all_prefixes(solution: &Solution) -> Vec<String> {
    solution.projects.iter().flat_map(prefixes).collect()
}

fn project_for<'a>(solution: &'a Solution, name: &str) -> Option<&'a Project> {
    solution
        .projects
        .iter()
        .filter(|project| {
            prefixes(project)
                .iter()
                .any(|prefix| name.starts_with(prefix))
        })
        .max_by_key(|project| {
            prefixes(project)
                .iter()
                .filter(|prefix| name.starts_with(prefix.as_str()))
                .map(String::len)
                .max()
                .unwrap_or(0)
        })
}

fn files_for(discovered: &[TableFile], table: Option<&str>) -> Vec<TableFile> {
    discovered
        .iter()
        .filter(|file| table.is_none_or(|table| file.table == table))
        .cloned()
        .collect()
}

fn server_tokens<R: Remote + ?Sized>(
    remote: &R,
    tables: &[String],
) -> Result<BTreeMap<String, Vec<Token>>, LocalizationError> {
    let replies = parallel::map(tables, |table| remote.tokens(table));
    tables
        .iter()
        .cloned()
        .zip(replies)
        .map(|(table, result)| result.map(|tokens| (table, tokens)).map_err(Into::into))
        .collect()
}

/// Compare local files with the server without changing either side.
pub fn status<R: Remote + ?Sized>(
    solution: &Solution,
    remote: &R,
    table: Option<&str>,
) -> Result<Status, LocalizationError> {
    check_prefixes(solution)?;
    let found = discover(&root(solution))?;
    let mut tables = remote.tables()?;
    tables.sort();
    if let Some(table) = table {
        if !found.files.iter().any(|file| file.table == table) && !tables.iter().any(|t| t == table)
        {
            return Err(LocalizationError::Arguments(format!(
                "localization table {table} is neither in this solution nor on the server"
            )));
        }
    }
    let files = files_for(&found.files, table);
    // Every server table is read, not only those with a file: a table the server holds tokens of
    // this solution in and no file has is what pull would create, and status must say so.
    let server_tables: Vec<String> = tables
        .iter()
        .filter(|name| table.is_none_or(|table| *name == table))
        .cloned()
        .collect();
    let server = server_tokens(remote, &server_tables)?;
    let mut missing_tables: Vec<String> = files
        .iter()
        .map(|file| file.table.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|name| !tables.contains(name))
        .collect();
    missing_tables.sort();
    Ok(Status {
        compared: with_unfiled(
            compare(&files, &server, &all_prefixes(solution)),
            &files,
            &server,
            solution,
        ),
        problems: problems(&files, &all_prefixes(solution)),
        missing_tables,
        unreadable: found.unreadable,
    })
}

/// `compared` plus, for each server table no file holds, its tokens under this solution's
/// prefixes as server only, in `compare`'s order.
fn with_unfiled(
    mut compared: Vec<Compared>,
    files: &[TableFile],
    server: &BTreeMap<String, Vec<Token>>,
    solution: &Solution,
) -> Vec<Compared> {
    for (table, tokens) in server {
        if files.iter().any(|file| &file.table == table) {
            continue;
        }
        for token in tokens {
            if project_for(solution, &token.name).is_some() {
                compared.push(Compared {
                    table: table.clone(),
                    name: token.name.clone(),
                    state: State::ServerOnly,
                    local: None,
                    server: Some(token.clone()),
                    file: None,
                });
            }
        }
    }
    compared.sort_by(|a, b| {
        (a.table != DEFAULT_TABLE, &a.table, &a.name).cmp(&(
            b.table != DEFAULT_TABLE,
            &b.table,
            &b.name,
        ))
    });
    compared
}

/// Two projects claiming the same prefix would make a token's owner depend on their order.
fn check_prefixes(solution: &Solution) -> Result<(), LocalizationError> {
    let mut owners: BTreeMap<String, &str> = BTreeMap::new();
    for project in &solution.projects {
        for prefix in prefixes(project) {
            if let Some(other) = owners.insert(prefix.clone(), &project.name) {
                if other != project.name {
                    return Err(LocalizationError::Arguments(format!(
                        "projects {other} and {} both claim localization prefix {prefix}; give each its own in [project.localization] prefixes",
                        project.name
                    )));
                }
            }
        }
    }
    Ok(())
}

/// The files with only the tokens `project` owns, so `target` does not mistake a file of a
/// project with a longer, overlapping prefix for this project's.
fn owned_by(files: &[TableFile], solution: &Solution, project: &Project) -> Vec<TableFile> {
    files
        .iter()
        .map(|file| TableFile {
            tokens: file
                .tokens
                .iter()
                .filter(|token| {
                    project_for(solution, &token.name)
                        .is_some_and(|owner| owner.name == project.name)
                })
                .cloned()
                .collect(),
            ..file.clone()
        })
        .collect()
}

/// Refuse when any problem `blocks`, naming up to five of them.
fn refuse(
    files: &[TableFile],
    solution: &Solution,
    blocks: impl Fn(&Problem) -> bool,
    doing: &str,
) -> Result<(), LocalizationError> {
    let blocking: Vec<String> = problems(files, &all_prefixes(solution))
        .iter()
        .filter(|problem| blocks(problem))
        .map(|problem| match problem {
            Problem::Duplicate { table, name, files } => format!(
                "{table}/{name} is in {} rows ({})",
                files.len().max(2),
                files
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Problem::NotInDefault { table, name, file } => format!(
                "{table}/{name} ({}) is not in Default; the server refuses it",
                file.display()
            ),
            Problem::Untranslated { table, name } => format!("{table}/{name} is untranslated"),
        })
        .collect();
    if blocking.is_empty() {
        return Ok(());
    }
    Err(LocalizationError::Invalid {
        path: root(solution),
        why: format!(
            "{} problem(s) to resolve before {doing}: {}",
            blocking.len(),
            blocking.into_iter().take(5).collect::<Vec<_>>().join("; ")
        ),
    })
}

struct PlannedFile {
    change: FileChange,
    bytes: Vec<u8>,
}

/// Write each planned file atomically. A failure names the files already written: they stay
/// written, and the next run plans from them.
fn write_plans(plans: &[PlannedFile]) -> Result<(), LocalizationError> {
    let mut written: Vec<String> = Vec::new();
    for plan in plans {
        let result = (|| {
            if plan.change.created {
                if let Some(parent) = plan.change.path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
            }
            workspace::write_entity(&plan.change.path, &plan.bytes)
                .map_err(|error| std::io::Error::other(error.to_string()))
        })();
        if let Err(error) = result {
            let why = if written.is_empty() {
                error.to_string()
            } else {
                format!("{error}; already written: {}", written.join(", "))
            };
            return Err(LocalizationError::Io {
                path: plan.change.path.clone(),
                why,
            });
        }
        written.push(plan.change.path.display().to_string());
    }
    Ok(())
}

/// Pull server values into the files that own their token namespaces.
pub fn pull<R: Remote + ?Sized>(
    solution: &Solution,
    remote: &R,
    table: Option<&str>,
    prune: bool,
    apply: bool,
) -> Result<Pulled, LocalizationError> {
    check_prefixes(solution)?;
    let found = discover(&root(solution))?;
    let mut tables = remote.tables()?;
    tables.sort();
    let selected: Vec<String> = match table {
        Some(table) if tables.iter().any(|name| name == table) => vec![table.to_string()],
        Some(table) => {
            return Err(LocalizationError::Arguments(format!(
                "localization table {table} is not on the server"
            )))
        }
        None => tables,
    };
    let files = files_for(&found.files, table);
    refuse(
        &files,
        solution,
        |problem| matches!(problem, Problem::Duplicate { .. }),
        "pulling",
    )?;
    let server = server_tokens(remote, &selected)?;
    let compared = compare(&files, &server, &all_prefixes(solution));
    let mut edits: BTreeMap<PathBuf, Vec<Edit>> = BTreeMap::new();
    let mut creates: BTreeMap<PathBuf, (String, Vec<Token>)> = BTreeMap::new();
    for item in &compared {
        match item.state {
            State::Differs => {
                let mut token = item.server.clone().expect("differs has server token");
                token.value = token.value.trim().to_string();
                edits
                    .entry(item.file.clone().expect("local differs has file"))
                    .or_default()
                    .push(Edit::Set(token));
            }
            State::ServerOnly => {
                let token = item.server.clone().expect("server-only has token");
                let Some(project) = project_for(solution, &token.name) else {
                    continue;
                };
                let path = target(
                    &owned_by(&files, solution, project),
                    &root(solution),
                    project,
                    &prefixes(project),
                    &item.table,
                    solution.projects.len() == 1,
                );
                let mut token = token;
                token.value = token.value.trim().to_string();
                if path.exists() {
                    edits.entry(path).or_default().push(Edit::Set(token));
                } else {
                    creates
                        .entry(path)
                        .or_insert_with(|| (item.table.clone(), Vec::new()))
                        .1
                        .push(token);
                }
            }
            State::LocalOnly if prune && selected.contains(&item.table) => {
                edits
                    .entry(item.file.clone().expect("local-only has file"))
                    .or_default()
                    .push(Edit::Remove(item.name.clone()));
            }
            _ => {}
        }
    }
    // `compare` deliberately starts with local table files, so a server table with no local
    // file is absent from it.  Pull is the operation that materializes exactly those tables.
    for table in &selected {
        if files.iter().any(|file| &file.table == table) {
            continue;
        }
        for token in server.get(table).into_iter().flatten() {
            let Some(project) = project_for(solution, &token.name) else {
                continue;
            };
            let path = target(
                &owned_by(&files, solution, project),
                &root(solution),
                project,
                &prefixes(project),
                table,
                solution.projects.len() == 1,
            );
            let mut token = token.clone();
            token.value = token.value.trim().to_string();
            creates
                .entry(path)
                .or_insert_with(|| (table.clone(), Vec::new()))
                .1
                .push(token);
        }
    }
    let mut plans = Vec::new();
    for (path, edits) in edits {
        let source = std::fs::read(&path).map_err(|error| LocalizationError::Io {
            path: path.clone(),
            why: error.to_string(),
        })?;
        let bytes = edit(&path, &source, &edits)?;
        let mut set = Vec::new();
        let mut removed = Vec::new();
        for edit in edits {
            match edit {
                Edit::Set(token) => set.push(token.name),
                Edit::Remove(name) => removed.push(name),
            }
        }
        set.sort();
        removed.sort();
        plans.push(PlannedFile {
            change: FileChange {
                path,
                created: false,
                set,
                removed,
            },
            bytes,
        });
    }
    for (path, (table, mut tokens)) in creates {
        tokens.sort();
        let header = remote.header(&table)?;
        let set = tokens.iter().map(|token| token.name.clone()).collect();
        plans.push(PlannedFile {
            change: FileChange {
                path,
                created: true,
                set,
                removed: Vec::new(),
            },
            bytes: render(&table, &header, &tokens),
        });
    }
    plans.sort_by(|a, b| a.change.path.cmp(&b.change.path));
    if apply {
        write_plans(&plans)?;
    }
    Ok(Pulled {
        files: plans.into_iter().map(|plan| plan.change).collect(),
        applied: apply,
    })
}

/// Import local table files and optionally remove remote tokens absent locally.
pub fn push<R: Remote>(
    solution: &Solution,
    remote: &R,
    table: Option<&str>,
    prune: bool,
    apply: bool,
    progress: &dyn Progress,
) -> Result<Pushed, LocalizationError> {
    check_prefixes(solution)?;
    let found = discover(&root(solution))?;
    if !found.unreadable.is_empty() {
        // An unreadable file may hold tokens this push would otherwise prune or leave stale.
        return Err(LocalizationError::Invalid {
            path: root(solution),
            why: format!(
                "{} table file(s) cannot be read; fix them before pushing: {}",
                found.unreadable.len(),
                found
                    .unreadable
                    .iter()
                    .take(5)
                    .map(|(path, why)| format!("{} ({why})", path.display()))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        });
    }
    let _reading = progress::phase(progress, "reading tables", None);
    let mut server_tables = remote.tables()?;
    server_tables.sort();
    let files = files_for(&found.files, table);
    let local_tables: BTreeSet<String> = files.iter().map(|file| file.table.clone()).collect();
    let selected: Vec<String> = match table {
        Some(table) => vec![table.to_string()],
        None => local_tables.iter().cloned().collect(),
    };
    // Default is read too: a language token needs it there, and a pruned Default token is
    // pruned from every language table that has it, so with --prune every table is read.
    let queried: Vec<String> = server_tables
        .iter()
        .filter(|name| prune || selected.contains(name) || name.as_str() == DEFAULT_TABLE)
        .cloned()
        .collect();
    let server = server_tokens(remote, &queried)?;
    drop(_reading);
    let compared: Vec<Compared> = compare(&files, &server, &all_prefixes(solution))
        .into_iter()
        .filter(|item| selected.contains(&item.table))
        .collect();
    let mut out = Vec::new();
    for table in selected {
        let table_files: Vec<&TableFile> =
            files.iter().filter(|file| file.table == table).collect();
        let mut set: Vec<String> = compared
            .iter()
            .filter(|item| {
                item.table == table && matches!(item.state, State::Differs | State::LocalOnly)
            })
            .map(|item| item.name.clone())
            .collect();
        let mut delete: Vec<String> = if prune {
            compared
                .iter()
                .filter(|item| item.table == table && item.state == State::ServerOnly)
                .map(|item| item.name.clone())
                .collect()
        } else {
            Vec::new()
        };
        set.sort();
        set.dedup();
        delete.sort();
        delete.dedup();
        let create = !server_tables.contains(&table) && !table_files.is_empty();
        if create || !set.is_empty() || !delete.is_empty() {
            out.push(TablePush {
                table,
                files: table_files.iter().map(|file| file.path.clone()).collect(),
                create,
                set,
                delete,
            });
        }
    }
    // A Default token pruned goes from every other table on the server too: the token services
    // refuse a language token Default lacks, and nothing would ever clean it up.
    let default_deletes: Vec<String> = out
        .iter()
        .find(|push| push.table == DEFAULT_TABLE)
        .map(|push| push.delete.clone())
        .unwrap_or_default();
    if !default_deletes.is_empty() {
        for (other, tokens) in &server {
            if other == DEFAULT_TABLE {
                continue;
            }
            let orphans: Vec<String> = default_deletes
                .iter()
                .filter(|name| tokens.iter().any(|token| &&token.name == name))
                .cloned()
                .collect();
            if orphans.is_empty() {
                continue;
            }
            match out.iter_mut().find(|push| &push.table == other) {
                Some(push) => {
                    push.delete.extend(orphans);
                    push.delete.sort();
                    push.delete.dedup();
                }
                None => out.push(TablePush {
                    table: other.clone(),
                    files: files
                        .iter()
                        .filter(|file| &file.table == other)
                        .map(|file| file.path.clone())
                        .collect(),
                    create: false,
                    set: Vec::new(),
                    delete: orphans,
                }),
            }
        }
    }
    out.sort_by(|a, b| {
        (a.table != DEFAULT_TABLE, &a.table).cmp(&(b.table != DEFAULT_TABLE, &b.table))
    });
    // The Importer takes a language token Default lacks, though nothing can resolve it: each
    // language token pushed must be in the server's Default or be pushed to Default now.
    let server_default: BTreeSet<&str> = server
        .get(DEFAULT_TABLE)
        .into_iter()
        .flatten()
        .map(|token| token.name.as_str())
        .collect();
    let default_set: BTreeSet<&str> = out
        .iter()
        .filter(|push| push.table == DEFAULT_TABLE)
        .flat_map(|push| push.set.iter().map(String::as_str))
        .collect();
    let lacking: Vec<String> = out
        .iter()
        .filter(|push| push.table != DEFAULT_TABLE)
        .flat_map(|push| {
            push.set
                .iter()
                .filter(|name| {
                    !server_default.contains(name.as_str()) && !default_set.contains(name.as_str())
                })
                .map(move |name| format!("{}/{name}", push.table))
        })
        .collect();
    if !lacking.is_empty() {
        return Err(LocalizationError::Invalid {
            path: root(solution),
            why: format!(
                "{} token(s) are not in the server's Default table; push Default too (without --table, or --table Default first): {}",
                lacking.len(),
                lacking.into_iter().take(5).collect::<Vec<_>>().join(", ")
            ),
        });
    }
    let pushed_tables: BTreeSet<&str> = out.iter().map(|push| push.table.as_str()).collect();
    refuse(
        &files,
        solution,
        |problem| match problem {
            Problem::Duplicate { table, .. } | Problem::NotInDefault { table, .. } => {
                pushed_tables.contains(table.as_str())
            }
            Problem::Untranslated { .. } => false,
        },
        "pushing",
    )?;
    if apply {
        // A table whose only change is a prune needs no import: the server already has its
        // tokens as the files say.
        let imported: Vec<&TablePush> = out
            .iter()
            .filter(|push| push.create || !push.set.is_empty())
            .collect();
        let import_count = imported.iter().map(|push| push.files.len() as u64).sum();
        let _imports = progress::phase(progress, "importing tables", Some(import_count));
        for push in imported {
            for path in &push.files {
                let bytes = std::fs::read(path).map_err(|error| LocalizationError::Io {
                    path: path.clone(),
                    why: error.to_string(),
                })?;
                imports::import_file(remote, &file_name(&push.table), &bytes, true, true, true)
                    .map_err(|error| match error {
                        imports::ImportError::Remote(error) => LocalizationError::Remote(error),
                        imports::ImportError::Invalid(why) => LocalizationError::Invalid {
                            path: root(solution),
                            why,
                        },
                    })?;
                progress.advance(1);
            }
        }
        drop(_imports);
        let mut deletes = out.iter().collect::<Vec<_>>();
        deletes.sort_by(|a, b| {
            (a.table == DEFAULT_TABLE, &a.table).cmp(&(b.table == DEFAULT_TABLE, &b.table))
        });
        let delete_count = deletes.iter().map(|push| push.delete.len() as u64).sum();
        let _deletes = progress::phase(progress, "deleting tokens", Some(delete_count));
        for push in deletes {
            for name in &push.delete {
                remote.delete_token(&push.table, name)?;
                progress.advance(1);
            }
        }
        drop(_deletes);
        let touched: Vec<String> = out.iter().map(|push| push.table.clone()).collect();
        let _confirm = progress::phase(progress, "confirming tables", Some(touched.len() as u64));
        let after = server_tokens(remote, &touched)?;
        for _ in &touched {
            progress.advance(1);
        }
        let check_files: Vec<TableFile> = files
            .into_iter()
            .filter(|file| touched.contains(&file.table))
            .collect();
        let failures: Vec<String> = compare(&check_files, &after, &all_prefixes(solution))
            .into_iter()
            .filter(|item| {
                matches!(item.state, State::Differs | State::LocalOnly)
                    || (prune && item.state == State::ServerOnly)
            })
            .map(|item| format!("{}/{}", item.table, item.name))
            .collect();
        let mut failures = failures;
        for push in &out {
            for name in &push.delete {
                if after
                    .get(&push.table)
                    .is_some_and(|tokens| tokens.iter().any(|token| &token.name == name))
                {
                    failures.push(format!("{}/{name}", push.table));
                }
            }
        }
        failures.sort();
        failures.dedup();
        if !failures.is_empty() {
            return Err(LocalizationError::NotVerified(format!(
                "server did not retain localization tokens: {}",
                failures.into_iter().take(10).collect::<Vec<_>>().join(", ")
            )));
        }
    }
    Ok(Pushed {
        tables: out,
        applied: apply,
    })
}

fn project<'a>(
    solution: &'a Solution,
    requested: Option<&str>,
    token: Option<&str>,
) -> Result<&'a Project, LocalizationError> {
    if let Some(name) = requested {
        return solution
            .projects
            .iter()
            .find(|project| project.name == name)
            .ok_or_else(|| LocalizationError::Arguments(format!("unknown project {name}")));
    }
    if let Some(token) = token {
        if let Some(project) = project_for(solution, token) {
            return Ok(project);
        }
        let choices = solution
            .projects
            .iter()
            .flat_map(prefixes)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(LocalizationError::Arguments(format!(
            "{token} matches no project prefix ({choices}); name it under one or pass --project"
        )));
    }
    if solution.projects.len() == 1 {
        Ok(&solution.projects[0])
    } else {
        Err(LocalizationError::Arguments(format!(
            "name a project: {}",
            solution
                .projects
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )))
    }
}

/// Create a table file for one project, seeded from its Default tokens.
pub fn new(
    solution: &Solution,
    table: &str,
    project_name: Option<&str>,
    header: Header,
    apply: bool,
) -> Result<Edited, LocalizationError> {
    check_prefixes(solution)?;
    let found = discover(&root(solution))?;
    let project = project(solution, project_name, None)?;
    let path = root(solution).join(&project.name).join(file_name(table));
    if path.exists() {
        return Err(LocalizationError::AlreadyExists(format!(
            "{} already exists",
            path.display()
        )));
    }
    let target_path = target(
        &owned_by(&found.files, solution, project),
        &root(solution),
        project,
        &prefixes(project),
        table,
        solution.projects.len() == 1,
    );
    if target_path.exists()
        && (target_path.starts_with(root(solution).join(&project.name))
            || found.files.iter().any(|file| {
                file.path == target_path
                    && file.tokens.iter().any(|token| {
                        project_for(solution, &token.name)
                            .is_some_and(|owner| owner.name == project.name)
                    })
            }))
    {
        return Err(LocalizationError::AlreadyExists(format!(
            "{} already holds this project's table",
            target_path.display()
        )));
    }
    let mut tokens: Vec<Token> = found
        .files
        .iter()
        .filter(|file| file.table == DEFAULT_TABLE)
        .flat_map(|file| file.tokens.iter())
        .filter(|token| {
            project_for(solution, &token.name).is_some_and(|owner| owner.name == project.name)
        })
        .cloned()
        .collect();
    tokens.sort();
    let plan = PlannedFile {
        change: FileChange {
            path,
            created: true,
            set: tokens.iter().map(|token| token.name.clone()).collect(),
            removed: Vec::new(),
        },
        bytes: render(table, &header, &tokens),
    };
    if apply {
        write_plans(std::slice::from_ref(&plan))?;
    }
    Ok(Edited {
        files: vec![plan.change],
        applied: apply,
    })
}

/// Set a token in a local table.
#[allow(clippy::too_many_arguments)] // Public command inputs are deliberately explicit.
pub fn set(
    solution: &Solution,
    name: &str,
    value: &str,
    table: Option<&str>,
    usage: Option<&str>,
    context: Option<&str>,
    project_name: Option<&str>,
    apply: bool,
) -> Result<Edited, LocalizationError> {
    check_prefixes(solution)?;
    if value != value.trim() {
        return Err(LocalizationError::Arguments(
            "a localization value cannot start or end with whitespace; the server trims it"
                .to_string(),
        ));
    }
    let found = discover(&root(solution))?;
    let table = table.unwrap_or(DEFAULT_TABLE);
    if table != DEFAULT_TABLE
        && !found
            .files
            .iter()
            .filter(|file| file.table == DEFAULT_TABLE)
            .any(|file| file.tokens.iter().any(|token| token.name == name))
    {
        return Err(LocalizationError::Invalid {
            path: root(solution),
            why: format!("{name} is not in Default; add it to Default first"),
        });
    }
    let holding = found
        .files
        .iter()
        .find(|file| file.table == table && file.tokens.iter().any(|token| token.name == name));
    // A token a file already holds stays where it is, whatever its prefix; only a new one needs
    // a project to decide its file.
    let path = match holding {
        Some(file) => file.path.clone(),
        None => {
            let project = project(solution, project_name, Some(name))?;
            target(
                &owned_by(&found.files, solution, project),
                &root(solution),
                project,
                &prefixes(project),
                table,
                solution.projects.len() == 1,
            )
        }
    };
    if !path.exists() && table != DEFAULT_TABLE {
        return Err(LocalizationError::Arguments(format!(
            "no file for table {table}; run `twaco localization new {table}`"
        )));
    }
    let previous = holding.and_then(|file| file.tokens.iter().find(|token| token.name == name));
    let token = Token {
        name: name.to_string(),
        value: value.to_string(),
        usage: usage
            .map(str::to_string)
            .or_else(|| previous.map(|token| token.usage.clone()))
            .unwrap_or_else(|| "label".to_string()),
        context: context
            .map(str::to_string)
            .or_else(|| previous.map(|token| token.context.clone()))
            .unwrap_or_default(),
    };
    let created = !path.exists();
    let bytes = if created {
        render(table, &Header::default(), std::slice::from_ref(&token))
    } else {
        let source = std::fs::read(&path).map_err(|error| LocalizationError::Io {
            path: path.clone(),
            why: error.to_string(),
        })?;
        edit(&path, &source, &[Edit::Set(token.clone())])?
    };
    let plan = PlannedFile {
        change: FileChange {
            path,
            created,
            set: vec![name.to_string()],
            removed: Vec::new(),
        },
        bytes,
    };
    if apply {
        write_plans(std::slice::from_ref(&plan))?;
    }
    Ok(Edited {
        files: vec![plan.change],
        applied: apply,
    })
}

/// Remove a token from its local table files.
pub fn remove(
    solution: &Solution,
    name: &str,
    table: Option<&str>,
    apply: bool,
) -> Result<Edited, LocalizationError> {
    let found = discover(&root(solution))?;
    // Removing from Default removes from every table: a language token Default lacks is one the
    // server refuses.
    let every_table = table.is_none_or(|table| table == DEFAULT_TABLE);
    let files: Vec<&TableFile> = found
        .files
        .iter()
        .filter(|file| {
            (every_table || Some(file.table.as_str()) == table)
                && file.tokens.iter().any(|token| token.name == name)
        })
        .collect();
    if files.is_empty() {
        return Err(LocalizationError::Unknown(format!(
            "no localization token named {name}"
        )));
    }
    let mut plans = Vec::new();
    for file in files {
        let source = std::fs::read(&file.path).map_err(|error| LocalizationError::Io {
            path: file.path.clone(),
            why: error.to_string(),
        })?;
        let bytes = edit(&file.path, &source, &[Edit::Remove(name.to_string())])?;
        plans.push(PlannedFile {
            change: FileChange {
                path: file.path.clone(),
                created: false,
                set: Vec::new(),
                removed: vec![name.to_string()],
            },
            bytes,
        });
    }
    if apply {
        write_plans(&plans)?;
    }
    Ok(Edited {
        files: plans.into_iter().map(|plan| plan.change).collect(),
        applied: apply,
    })
}
