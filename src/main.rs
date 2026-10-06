//! twaco: a portable ThingWorx sidecar toolchain.
//!
//! This is an adapter. Every decision lives in `twaco::core`, so the MCP server added later
//! answers the same questions the same way rather than growing its own opinions.
//!
//! **Exit codes are three, not two.** 0 is success, 1 is "there is work to do" from a `--check`,
//! and 2 is a failure: bad arguments, bad configuration, or a file that would not read or write.
//! Automation has to be able to tell drift from breakage, and one code for both hides an outage
//! behind a diff.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use twaco::core::commands::{self, Mode};
use twaco::core::config::Solution;
use twaco::core::index::Confidence;
use twaco::core::{
    adopt, backup, catalog, config_table, datatable_copy, db, deploy, docs, entity_carry,
    entity_delete, impact, lock, newblock, profile, push, relocate, rename, retemplate, server,
    status, types, unused, workflow, workspace,
};

mod cli;

use cli::args::Args;
use cli::content::{export_cmd, ext_cmd, import_cmd, package_cmd, repo_cmd};
use cli::data::{call, config_table, datatable_copy_cmd, db_clean_cmd, db_cmd, logs_cmd};
use cli::entity::{
    entity_carry_cmd, entity_delete_cmd, entity_get, entity_push, entity_restore_cmd, entity_status,
};
use cli::info::{
    catalog_cmd, docs_cmd, guide_cmd, help_cmd, impact_cmd, javadoc_cmd, settings_cmd, unused_cmd,
    write_agent_files,
};
use cli::refactor::{adopt_cmd, new_building_block_cmd, relocate_cmd, rename_cmd, retemplate_cmd};
use cli::source::{bundle, check, deploy_cmd, extract, fmt, sync_cmd, types_cmd};
use cli::usage::usage;

/// Success.
const OK: u8 = 0;
/// A `--check` found work to do. Not an error.
const DRIFT: u8 = 1;
/// Something failed: arguments, configuration, or I/O.
const FAILED: u8 = 2;

fn main() -> ExitCode {
    quiet_when_the_reader_leaves();
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Only as the command itself: `--version` is also a flag of `help`, naming a help release.
    if args.first().is_some_and(|a| a == "--version" || a == "-V") {
        println!("twaco {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::from(OK);
    }

    let Some(command) = args.first().map(String::as_str) else {
        usage();
        return ExitCode::from(FAILED);
    };

    // The help center needs no solution, so it runs before discovery and uses one if found.
    if command == "help" {
        return ExitCode::from(help_cmd(&args[1..]));
    }
    // The public Java API documentation also needs no solution.
    if command == "javadoc" {
        return ExitCode::from(javadoc_cmd(&args[1..]));
    }
    // So does the guide: its built-in topics are the same everywhere.
    if command == "guide" {
        return ExitCode::from(guide_cmd(&args[1..]));
    }

    // init makes the config discovery would look for, so it runs before discovery too.
    if command == "init" {
        let here = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let write = match &args[1..] {
            [] => false,
            [flag] if flag == "--write" => true,
            // The agent files alone, for a solution that already has its config.
            [flag] if flag == "--agents" => {
                let solution = match Solution::discover(&here) {
                    Ok(solution) => solution,
                    Err(error) => {
                        eprintln!("twaco: {error}");
                        return ExitCode::from(FAILED);
                    }
                };
                return ExitCode::from(write_agent_files(&solution));
            }
            _ => {
                eprintln!("twaco: init takes --write or --agents");
                return ExitCode::from(FAILED);
            }
        };
        let proposal = twaco::core::init::propose(&here);
        for note in &proposal.notes {
            eprintln!("twaco: {note}");
        }
        if proposal.projects == 0 {
            return ExitCode::from(FAILED);
        }
        let target = here.join(twaco::core::config::CONFIG_FILE);
        if !write {
            print!("{}", proposal.toml);
            eprintln!("twaco: nothing written; `twaco init --write` creates {}, and AGENTS.md and CLAUDE.md where absent", target.display());
            return ExitCode::from(OK);
        }
        // Created new, never renamed over: whatever is at the name, a file or a link, stays.
        let created = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .and_then(|mut file| std::io::Write::write_all(&mut file, proposal.toml.as_bytes()));
        match created {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                eprintln!(
                    "twaco: {} exists and is never overwritten; remove it first to start again",
                    target.display()
                );
                return ExitCode::from(FAILED);
            }
            Err(error) => {
                eprintln!("twaco: {}: {error}", target.display());
                return ExitCode::from(FAILED);
            }
        }
        // Prove it loads, so a proposal twaco cannot read is never left behind silently.
        return match Solution::load(&target) {
            Ok(solution) => {
                println!(
                    "wrote {} with {} project(s); `twaco doctor` checks the rest",
                    target.display(),
                    solution.projects.len()
                );
                ExitCode::from(write_agent_files(&solution))
            }
            Err(error) => {
                eprintln!("twaco: the written config does not load: {error}");
                ExitCode::from(FAILED)
            }
        };
    }

    // doctor diagnoses a missing solution rather than failing on it, so it runs before discovery.
    if command == "doctor" {
        let mut profile_name = "default".to_string();
        let mut rest = args.iter().skip(1);
        while let Some(arg) = rest.next() {
            match (arg.as_str(), rest.next()) {
                ("--profile", Some(name)) => profile_name = name.clone(),
                _ => {
                    eprintln!("twaco: doctor takes only --profile <name>");
                    return ExitCode::from(FAILED);
                }
            }
        }
        let here = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let items = twaco::core::doctor::diagnose(&here, &profile_name);
        for item in &items {
            let mark = match item.health {
                twaco::core::doctor::Health::Ok => "ok  ",
                twaco::core::doctor::Health::Warn => "warn",
                twaco::core::doctor::Health::Fail => "FAIL",
            };
            println!("  {mark}  {:<15} {}", item.subject, item.detail);
        }
        let failed = items
            .iter()
            .any(|i| i.health == twaco::core::doctor::Health::Fail);
        return ExitCode::from(if failed { FAILED } else { OK });
    }

    // The MCP server finds the solution on every call rather than at start, so it starts in a
    // directory without one and each tool says so, and an edit to twaco.toml needs no restart.
    if command == "mcp" {
        if args.len() > 1 {
            eprintln!("twaco: mcp takes no arguments; set TWACO_ROOT to serve another directory");
            return ExitCode::from(FAILED);
        }
        let root = std::env::var_os("TWACO_ROOT")
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let stdin = std::io::stdin();
        return match twaco::mcp::serve(&root, stdin.lock(), std::io::stdout().lock()) {
            Ok(()) => ExitCode::from(OK),
            Err(error) => {
                eprintln!("twaco: mcp: {error}");
                ExitCode::from(FAILED)
            }
        };
    }

    let (route, argument_start, known) = match route(&args) {
        Ok(route) => route,
        Err(why) => {
            eprintln!("twaco: {why}");
            usage();
            return ExitCode::from(FAILED);
        }
    };

    let parsed = match Args::parse(&args[argument_start..], known) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("twaco: {e}");
            usage();
            return ExitCode::from(FAILED);
        }
    };

    let code = run(|solution| {
        // Held until this closure returns, so for the whole command. See core::lock.
        let _lock = if writes_workspace(route, &parsed) {
            match take_lock(solution, route) {
                Ok(lock) => Some(lock),
                Err(code) => return code,
            }
        } else {
            None
        };
        match route {
            "projects" => projects(solution),
            "types" => types_cmd(solution, &parsed),
            "extract" => extract(solution, &parsed),
            "sync" => sync_cmd(solution, &parsed),
            "fmt" => fmt(solution, &parsed),
            "check" => check(solution, &parsed),
            "bundle" => bundle(solution, &parsed),
            "deploy" => deploy_cmd(solution, &parsed),
            "call" => call(solution, &parsed),
            "repo" => repo_cmd(solution, &parsed),
            "ext" => ext_cmd(solution, &parsed),
            "settings" => settings_cmd(solution, &parsed),
            "catalog" => catalog_cmd(solution, &parsed),
            "impact" => impact_cmd(solution, &parsed),
            "unused" => unused_cmd(solution, &parsed),
            "docs" => docs_cmd(solution, &parsed),
            "package" => package_cmd(solution, &parsed),
            "export" => export_cmd(solution, &parsed),
            "import" => import_cmd(solution, &parsed),
            "logs" => logs_cmd(solution, &parsed),
            "adopt" => adopt_cmd(solution, &parsed),
            "rename entity" | "rename prefix" | "rename field" | "rename service"
            | "rename param" | "rename table" | "rename property" => {
                rename_cmd(solution, route, &parsed)
            }
            "move service" | "move property" | "copy service" | "copy property" => {
                relocate_cmd(solution, route, &parsed)
            }
            "retemplate" => retemplate_cmd(solution, &parsed),
            "new building-block" => new_building_block_cmd(solution, &parsed),
            "config-table" => config_table(solution, &parsed),
            "entity get" => entity_get(solution, &parsed),
            "entity status" => entity_status(solution, &parsed),
            "entity push" => entity_push(solution, &parsed),
            "entity delete" => entity_delete_cmd(solution, &parsed),
            "entity carry" => entity_carry_cmd(solution, &parsed),
            "entity restore" => entity_restore_cmd(solution, &parsed),
            "db run" | "db query" => db_cmd(solution, route, &parsed),
            "db clean" => db_clean_cmd(solution, &parsed),
            "datatable copy" => datatable_copy_cmd(solution, &parsed),
            _ => unreachable!("the command was checked above"),
        }
    });
    ExitCode::from(code)
}

/// A command's route, where its arguments start, and the flags it accepts.
///
/// Flags are validated per command before anything is read or written. An unrecognised flag
/// used to be ignored, which turned `sync --cehck` -- a typo -- into a real write.
type Route = (&'static str, usize, &'static [&'static str]);

fn route(args: &[String]) -> Result<Route, String> {
    let command = args.first().map(String::as_str).unwrap_or_default();
    Ok(match command {
        "projects" => ("projects", 1, &[]),
        "types" => ("types", 1, &["--platform", "--check", "--json", "--profile"]),
        "extract" => ("extract", 1, &["--all", "--project"]),
        "sync" => (
            "sync",
            1,
            &["--all", "--check", "--allow-add-remove", "--relayout", "--project"],
        ),
        "fmt" => ("fmt", 1, &["--check"]),
        "check" => ("check", 1, &["--detail", "--live", "--profile"]),
        "bundle" => ("bundle", 1, &["--backend-only", "--check"]),
        "deploy" => (
            "deploy",
            1,
            &[
                "--apply", "--force", "--backend-only", "--only", "--only-projects",
                "--skip-checks", "--no-backup", "--profile",
            ],
        ),
        "call" => ("call", 1, &["--timeout", "--detail", "--profile", "--with-logs"]),
        "ext" => ("ext", 1, &["--apply", "--json", "--profile"]),
        "settings" => ("settings", 1, &["--search", "--json", "--profile"]),
        "catalog" => ("catalog", 1, &["--project", "--search", "--json"]),
        "impact" => (
            "impact",
            1,
            &["--member", "--min-confidence", "--depth", "--detail", "--json", "--dot"],
        ),
        "unused" => (
            "unused",
            1,
            &["--min-confidence", "--collection", "--detail", "--json"],
        ),
        "docs" => ("docs", 1, &["--detail", "--json", "--out", "--force"]),
        "package" => ("package", 1, &["--project", "--backend-only", "--frontend-only", "--editable", "--out", "--force"]),
        "import" => (
            "import",
            1,
            &["--apply", "--overwrite-properties", "--overwrite-tables", "--repository", "--path", "--profile", "--detail"],
        ),
        "export" => (
            "export",
            1,
            &["--out", "--force", "--project", "--profile", "--repository", "--path", "--collection", "--tags", "--zip", "--with-dependents", "--apply"],
        ),
        "repo" => ("repo", 1, &["--recursive", "--out", "--json", "--profile", "--force", "--overwrite", "--apply"]),
        "logs" => (
            "logs",
            1,
            &[
                "--since", "--from", "--to", "--level", "--grep", "--regex", "--user", "--thread", "--origin",
                "--limit", "--oldest-first", "--json", "--profile", "--sublogger", "--reset", "--apply",
            ],
        ),
        "adopt" => ("adopt", 1, &["--entity", "--detail", "--json", "--fail-on-revert", "--apply"]),
        "retemplate" => ("retemplate", 1, &["--to", "--add-shapes", "--remove-shapes", "--accept-loss", "--apply", "--detail", "--json"]),
        "new" => match args.get(1).map(String::as_str) {
            Some("building-block") => (
                "new building-block",
                2,
                &["--type", "--display-name", "--description", "--parent", "--root", "--base-extension", "--model-logic", "--no-management-shape", "--apply", "--json"],
            ),
            Some(other) => return Err(format!("unknown new command `{other}`")),
            None => return Err("new needs `building-block`".to_string()),
        },
        "move" => match args.get(1).map(String::as_str) {
            Some("service") => ("move service", 2, &["--as", "--leave-delegate", "--apply", "--detail", "--json"]),
            Some("property") => ("move property", 2, &["--as", "--apply", "--detail", "--json"]),
            Some(other) => return Err(format!("unknown move command `{other}`")),
            None => return Err("move needs `service` or `property`".to_string()),
        },
        "copy" => match args.get(1).map(String::as_str) {
            Some("service") => ("copy service", 2, &["--as", "--apply", "--detail", "--json"]),
            Some("property") => ("copy property", 2, &["--as", "--apply", "--detail", "--json"]),
            Some(other) => return Err(format!("unknown copy command `{other}`")),
            None => return Err("copy needs `service` or `property`".to_string()),
        },
        "rename" => match args.get(1).map(String::as_str) {
            Some("entity") => ("rename entity", 2, &["--apply", "--text", "--detail", "--json", "--skip-checks", "--sql", "--sql-dir", "--no-sql"]),
            Some("prefix") => ("rename prefix", 2, &["--apply", "--text", "--detail", "--json", "--skip-checks", "--sql", "--sql-dir", "--no-sql"]),
            Some("field") => ("rename field", 2, &["--apply", "--text", "--detail", "--json", "--skip-checks", "--sql", "--sql-dir", "--no-sql"]),
            Some("service") => ("rename service", 2, &["--apply", "--text", "--detail", "--json", "--skip-checks"]),
            Some("param") => ("rename param", 2, &["--apply", "--text", "--detail", "--json", "--skip-checks"]),
            Some("table") => ("rename table", 2, &["--apply", "--text", "--detail", "--json", "--skip-checks"]),
            Some("property") => ("rename property", 2, &["--apply", "--text", "--detail", "--json", "--skip-checks"]),
            Some(other) => return Err(format!("unknown rename command `{other}`")),
            None => return Err("rename needs `entity`, `prefix`, `field`, `service`, `param`, `table` or `property`".to_string()),
        },
        "config-table" => (
            "config-table",
            1,
            &["--backup", "--restore", "--apply", "--diff", "--detail", "--profile"],
        ),
        "entity" => match args.get(1).map(String::as_str) {
            Some("get") => ("entity get", 2, &["--out", "--profile"]),
            Some("push") => ("entity push", 2, &["--apply", "--force", "--no-backup", "--profile"]),
            Some("delete") => ("entity delete", 2, &["--renamed", "--force", "--allow-repository-defined", "--allow-outside-dependents", "--allow-file-repository-data-loss", "--apply", "--no-backup", "--profile", "--json"]),
            Some("restore") => ("entity restore", 2, &["--apply", "--profile", "--json"]),
            Some("carry") => ("entity carry", 2, &["--renamed", "--apply", "--detail", "--profile", "--json"]),
            Some("status") => {
                ("entity status", 2, &["--all", "--project", "--profile", "--detail", "--record"])
            }
            Some(other) => return Err(format!("unknown entity command `{other}`")),
            None => return Err("entity needs `get`, `status`, `push`, `delete`, `carry` or `restore`".to_string()),
        },
        "datatable" => match args.get(1).map(String::as_str) {
            Some("copy") => ("datatable copy", 2, &["--map", "--drop-unmapped", "--append", "--max-rows", "--apply", "--profile", "--json"]),
            Some(other) => return Err(format!("unknown datatable command `{other}`")),
            None => return Err("datatable needs `copy`".to_string()),
        },
        "db" => match args.get(1).map(String::as_str) {
            Some("run") => ("db run", 2, &["--thing", "--no-transaction", "--timeout", "--apply", "--profile", "--json"]),
            Some("query") => ("db query", 2, &["-q", "--thing", "--max-rows", "--timeout", "--profile", "--detail", "--json"]),
            Some("clean") => ("db clean", 2, &["--apply", "--profile", "--json"]),
            Some(other) => return Err(format!("unknown db command `{other}`")),
            None => return Err("db needs `run`, `query` or `clean`".to_string()),
        },
        other => return Err(format!("unknown command `{other}`")),
    })
}

/// Exit quietly when stdout's reader has gone, as `twaco projects | head` does once it has its
/// lines. `println!` panics on a closed pipe, and a panic there is noise. The exit is 141, what
/// a shell reports for a process its pipe closed: the command's own result is unknown at this
/// point, so it must not read as success.
fn quiet_when_the_reader_leaves() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info.payload();
        let message = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or_default();
        // Broken pipe: os error 32 on Unix; ERROR_NO_DATA (232) or ERROR_BROKEN_PIPE (109) on
        // Windows.
        let closed = ["(os error 32)", "(os error 232)", "(os error 109)"]
            .iter()
            .any(|code| message.contains(code));
        if message.starts_with("failed printing to stdout") && closed {
            std::process::exit(141);
        }
        default(info);
    }));
}

/// Load the solution once, then hand it to a command.
/// Whether a command changes the workspace: its entity files, sidecars, bundle or baseline.
///
/// Those take the workspace lock. Everything else runs alongside them, so `entity status` can
/// watch a deploy in progress. `config-table` writes only to the server and to a backup file of
/// the user's choosing, and `entity get --out` writes a file the user named, so neither does.
fn writes_workspace(route: &str, _args: &Args) -> bool {
    match route {
        // Even with --apply, db run writes only a throwaway server Thing and needs no workspace lock.
        "db run" => false,
        // `repo pull` takes its own lock before discovering the tree it will write.
        "repo" => false,
        _ => false,
    }
}

/// Take the lock, sweeping stale temporaries from everywhere twaco writes through one.
fn take_lock(solution: &Solution, route: &str) -> Result<lock::WorkspaceLock, u8> {
    match lock::acquire_for(solution, route) {
        Ok(lock) => {
            for path in &lock.recovered {
                eprintln!(
                    "twaco: removed {}, left by an interrupted write",
                    path.display()
                );
            }
            for line in &lock.recovery {
                eprintln!("twaco: {line}");
            }
            Ok(lock)
        }
        Err(error) => {
            eprintln!("twaco: {error}");
            Err(FAILED)
        }
    }
}

/// What an executor reported about taking the workspace lock, as the lines `take_lock` prints.
fn print_notices(notices: &commands::Notices) {
    for line in notices.lines() {
        eprintln!("twaco: {line}");
    }
}

fn run(command: impl FnOnce(&Solution) -> u8) -> u8 {
    let here = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match Solution::discover(&here) {
        Ok(solution) => command(&solution),
        Err(e) => {
            eprintln!("twaco: {e}");
            FAILED
        }
    }
}

fn projects(solution: &Solution) -> u8 {
    let order = match solution.deploy_order() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("twaco: {e}");
            return FAILED;
        }
    };
    let label = if solution.solution.name.is_empty() {
        "(unnamed)"
    } else {
        &solution.solution.name
    };
    println!("solution {label}  root {}", solution.root.display());
    println!("sidecars {}", solution.src_root().display());
    println!();
    println!("deploy order:");
    for (index, project) in order.iter().enumerate() {
        let root = solution.project_root(project);
        let missing = if root.is_dir() { "" } else { "   [missing]" };
        println!("  {}. {}", index + 1, project.name);
        println!("       root {}{}", root.display(), missing);
        if !project.depends_on.is_empty() {
            println!("       after {}", project.depends_on.join(", "));
        }
    }

    let found = workspace::discover(solution);
    println!();
    println!("{} entity document(s)", found.entities.len());

    // An entity whose own projectName disagrees with the folder it sits in imports into the
    // wrong project and nothing else notices, so it is reported here, where a person would look.
    let misfiled: Vec<&workspace::EntityFile> =
        found.entities.iter().filter(|e| e.is_misfiled()).collect();
    if !misfiled.is_empty() {
        println!();
        println!("{} misfiled entity document(s):", misfiled.len());
        for entity in &misfiled {
            println!(
                "  {} says {} but is filed under {}",
                entity.info.name, entity.info.project, entity.found_under
            );
        }
    }
    if !found.unreadable.is_empty() {
        eprintln!();
        eprintln!("{} file(s) could not be read:", found.unreadable.len());
        for problem in &found.unreadable {
            eprintln!("  {problem}");
        }
        return FAILED;
    }
    OK
}

/// The entity files a command should act on, narrowed by `--project` and then by name.
fn targets(
    solution: &Solution,
    args: &Args,
) -> Result<(Vec<workspace::EntityFile>, Vec<String>), String> {
    let found = workspace::discover(solution);
    let mut pool = found.entities;

    if let Some(wanted) = &args.project {
        if solution.project(wanted).is_none() {
            return Err(format!("this solution has no project named {wanted}"));
        }
        pool.retain(|e| &e.found_under == wanted);
    }

    if args.has("--all") {
        return Ok((pool, found.unreadable));
    }
    if args.names.is_empty() {
        return Err("name an entity, or pass --all".to_string());
    }
    let mut chosen = Vec::new();
    for name in &args.names {
        chosen.push(
            workspace::resolve(&pool, name)
                .map_err(|e| e.to_string())?
                .clone(),
        );
    }
    Ok((chosen, found.unreadable))
}
