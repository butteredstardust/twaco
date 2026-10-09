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
use cli::content::{export_cmd, ext_cmd, import_cmd, package_cmd, repo_cmd, search_cmd};
use cli::data::{call, config_table, datatable_copy_cmd, db_clean_cmd, db_cmd, logs_cmd};
use cli::entity::{
    entity_carry_cmd, entity_delete_cmd, entity_get, entity_push, entity_restore_cmd,
    entity_status, permissions_apply_cmd, permissions_audit_cmd, permissions_cmd,
    permissions_init_cmd,
};
use cli::info::{
    catalog_cmd, docs_cmd, guide_cmd, help_cmd, impact_cmd, javadoc_cmd, settings_cmd, unused_cmd,
    update_cmd, write_agent_files,
};
use cli::localization::localization_cmd;
use cli::refactor::{
    adopt_cmd, handoff_cmd, new_building_block_cmd, relocate_cmd, rename_cmd, retemplate_cmd,
};
use cli::source::{bundle, check, deploy_cmd, extract, fmt, sync_cmd, types_cmd};

/// Success.
const OK: u8 = 0;
/// A `--check` found work to do. Not an error.
const DRIFT: u8 = 1;
/// Something failed: arguments, configuration, or I/O.
const FAILED: u8 = 2;

fn main() -> ExitCode {
    quiet_when_the_reader_leaves();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = run_command(&args);
    update_notice(args.first().map(String::as_str).unwrap_or_default());
    code
}

/// After the command, a line on stderr when a newer release exists. The network is asked at
/// most once a day, with a short timeout, and never for the MCP server, CI or a redirected
/// stderr. See core::update.
fn update_notice(command: &str) {
    use std::io::IsTerminal;
    use twaco::core::update;
    let wanted = update::notice_wanted(
        command,
        &|name| std::env::var(name).ok(),
        std::io::stderr().is_terminal(),
    );
    if command.is_empty() || !wanted {
        return;
    }
    let Some(cache) = update::cache_file() else {
        return;
    };
    let web = update::Web::new(Duration::from_secs(2));
    let now = jiff::Timestamp::now().as_second();
    let current = env!("CARGO_PKG_VERSION");
    if let Some(line) = update::notice(
        &web,
        update::MANIFEST_URL,
        update::PUBLIC_KEY,
        &cache,
        now,
        current,
    ) {
        eprintln!("{} {line}", cli::style::prefix());
    }
}

fn run_command(args: &[String]) -> ExitCode {
    // clap refuses what no command takes, with a suggestion; it answers --help and --version
    // (exit 0) and a usage error (exit 2, FAILED) itself.
    let words = std::iter::once("twaco".to_string()).chain(args.iter().cloned());
    let matches = match cli::spec::tree().try_get_matches_from(words) {
        Ok(matches) => matches,
        Err(error) => {
            // Every twaco error starts `twaco:`, which people and scripts look for; help,
            // --version and the listing for a missing command are not errors and stay as clap
            // writes them.
            let plain = matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp
                    | clap::error::ErrorKind::DisplayVersion
                    | clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
            );
            if plain {
                let _ = error.print();
            } else {
                let text = error.render().to_string();
                eprint!(
                    "{} {}",
                    cli::style::prefix(),
                    text.strip_prefix("error: ").unwrap_or(&text)
                );
            }
            return ExitCode::from(u8::try_from(error.exit_code()).unwrap_or(FAILED));
        }
    };
    let (command, matched) = cli::spec::matched(&matches);
    let parsed = match cli::spec::args_of(command, matched) {
        Ok(parsed) => parsed,
        Err(why) => {
            eprintln!("{} {why}", cli::style::prefix());
            return ExitCode::from(FAILED);
        }
    };
    let route = command.path;
    let (log, log_file) = cli::spec::log_options(matched);
    if let Some(warning) = twaco::core::diagnostics::init(log.as_deref(), log_file.as_deref()) {
        eprintln!("{} {warning}", cli::style::prefix());
    }
    let span = tracing::info_span!("command", command = route);
    let _entered = span.enter();
    let started = std::time::Instant::now();
    let code = dispatch(route, &parsed);
    tracing::info!(
        exit = code,
        elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        "command finished"
    );
    ExitCode::from(code)
}

/// Run one parsed command and return its exit code.
fn dispatch(route: &str, parsed: &Args) -> u8 {
    match route {
        // The help center needs no solution, so it runs before discovery and uses one if found.
        "help" => return help_cmd(parsed),
        // The public Java API documentation also needs no solution.
        "javadoc" => return javadoc_cmd(parsed),
        // So does the guide: its built-in topics are the same everywhere.
        "guide" => return guide_cmd(parsed),
        // update replaces the binary, not the solution, so it needs none.
        "update" => return update_cmd(parsed),
        // init makes the config discovery would look for, so it runs before discovery too.
        "init" => return init_cmd(parsed),
        // doctor diagnoses a missing solution rather than failing on it.
        "doctor" => return doctor_cmd(parsed),
        "mcp" => return mcp_cmd(),
        _ => {}
    }

    run(|solution| {
        // Held until this closure returns, so for the whole command. See core::lock.
        let _lock = if writes_workspace(route, parsed) {
            match take_lock(solution, route) {
                Ok(lock) => Some(lock),
                Err(code) => return code,
            }
        } else {
            None
        };
        match route {
            "projects" => projects(solution),
            "types" => types_cmd(solution, parsed),
            "extract" => extract(solution, parsed),
            "sync" => sync_cmd(solution, parsed),
            "fmt" => fmt(solution, parsed),
            "check" => check(solution, parsed),
            "bundle" => bundle(solution, parsed),
            "deploy" => deploy_cmd(solution, parsed),
            "call" => call(solution, parsed),
            "repo" => repo_cmd(solution, parsed),
            "localization" => localization_cmd(solution, parsed),
            "ext" => ext_cmd(solution, parsed),
            "settings" => settings_cmd(solution, parsed),
            "catalog" => catalog_cmd(solution, parsed),
            "impact" => impact_cmd(solution, parsed),
            "unused" => unused_cmd(solution, parsed),
            "docs" => docs_cmd(solution, parsed),
            "package" => package_cmd(solution, parsed),
            "search" => search_cmd(solution, parsed),
            "export" => export_cmd(solution, parsed),
            "import" => import_cmd(solution, parsed),
            "logs" => logs_cmd(solution, parsed),
            "adopt" => adopt_cmd(solution, parsed),
            "handoff" => handoff_cmd(solution, parsed),
            "rename entity" | "rename prefix" | "rename field" | "rename service"
            | "rename param" | "rename table" | "rename property" => {
                rename_cmd(solution, route, parsed)
            }
            "move service" | "move property" | "copy service" | "copy property" => {
                relocate_cmd(solution, route, parsed)
            }
            "retemplate" => retemplate_cmd(solution, parsed),
            "new building-block" => new_building_block_cmd(solution, parsed),
            "config-table" => config_table(solution, parsed),
            "entity get" => entity_get(solution, parsed),
            "entity status" => entity_status(solution, parsed),
            "entity push" => entity_push(solution, parsed),
            "entity delete" => entity_delete_cmd(solution, parsed),
            "entity carry" => entity_carry_cmd(solution, parsed),
            "permissions diff" | "permissions push" => permissions_cmd(solution, route, parsed),
            "permissions audit" => permissions_audit_cmd(solution, parsed),
            "permissions apply" => permissions_apply_cmd(solution, parsed),
            "permissions init" => permissions_init_cmd(solution, parsed),
            "entity restore" => entity_restore_cmd(solution, parsed),
            "db run" | "db query" => db_cmd(solution, route, parsed),
            "db clean" => db_clean_cmd(solution, parsed),
            "datatable copy" => datatable_copy_cmd(solution, parsed),
            other => unreachable!("{other} is in COMMANDS but has no handler"),
        }
    })
}

/// `twaco init [--write|--agents]`: propose a twaco.toml from the repository's own entities.
fn init_cmd(parsed: &Args) -> u8 {
    let here = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let write = match (parsed.has("--write"), parsed.has("--agents")) {
        (false, false) => false,
        (true, false) => true,
        // The agent files alone, for a solution that already has its config.
        (false, true) => {
            return match Solution::discover(&here) {
                Ok(solution) => write_agent_files(&solution),
                Err(error) => {
                    eprintln!("{} {error}", cli::style::prefix());
                    FAILED
                }
            };
        }
        (true, true) => {
            eprintln!(
                "{} init takes --write or --agents, not both",
                cli::style::prefix()
            );
            return FAILED;
        }
    };
    let proposal = twaco::core::init::propose(&here);
    for note in &proposal.notes {
        eprintln!("{} {note}", cli::style::prefix());
    }
    if proposal.projects == 0 {
        return FAILED;
    }
    let target = here.join(twaco::core::config::CONFIG_FILE);
    if !write {
        print!("{}", proposal.toml);
        eprintln!("{} nothing written; `twaco init --write` creates {}, and AGENTS.md and CLAUDE.md where absent", cli::style::prefix(), target.display());
        return OK;
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
                "{} {} exists and is never overwritten; remove it first to start again",
                cli::style::prefix(),
                target.display()
            );
            return FAILED;
        }
        Err(error) => {
            eprintln!("{} {}: {error}", cli::style::prefix(), target.display());
            return FAILED;
        }
    }
    // Prove it loads, so a proposal twaco cannot read is never left behind silently.
    match Solution::load(&target) {
        Ok(solution) => {
            println!(
                "wrote {} with {} project(s); `twaco doctor` checks the rest",
                target.display(),
                solution.projects.len()
            );
            write_agent_files(&solution)
        }
        Err(error) => {
            eprintln!(
                "{} the written config does not load: {error}",
                cli::style::prefix()
            );
            FAILED
        }
    }
}

/// `twaco doctor [--profile <name>]`: what resolved, what is reachable, what is missing.
fn doctor_cmd(parsed: &Args) -> u8 {
    let profile_name = parsed
        .profile
        .clone()
        .unwrap_or_else(|| "default".to_string());
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
    if failed {
        FAILED
    } else {
        OK
    }
}

/// `twaco mcp`. The server finds the solution on every call rather than at start, so it starts in
/// a directory without one and each tool says so, and an edit to twaco.toml needs no restart.
fn mcp_cmd() -> u8 {
    let root = std::env::var_os("TWACO_ROOT")
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    let stdin = std::io::stdin();
    match twaco::mcp::serve(&root, stdin.lock(), std::io::stdout()) {
        Ok(()) => OK,
        Err(error) => {
            eprintln!("{} mcp: {error}", cli::style::prefix());
            FAILED
        }
    }
}

/// A command's path, how many words it takes, and the flags it accepts: the table's view, for
/// tests that start from words on a command line.
#[cfg(test)]
type Route = (&'static str, usize, &'static [&'static str]);

#[cfg(test)]
fn route(args: &[String]) -> Result<Route, String> {
    let two = args.iter().take(2).cloned().collect::<Vec<_>>().join(" ");
    let one = args.first().cloned().unwrap_or_default();
    for (path, words) in [(two, 2), (one, 1)] {
        if let Some(command) = cli::spec::command(&path) {
            return Ok((command.path, words, command.flags));
        }
    }
    Err(format!("unknown command `{}`", args.join(" ")))
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
        // `localization` takes its own lock before discovering the table files it will write.
        "localization" => false,
        _ => false,
    }
}

/// Take the lock, sweeping stale temporaries from everywhere twaco writes through one.
fn take_lock(solution: &Solution, route: &str) -> Result<lock::WorkspaceLock, u8> {
    match lock::acquire_for(solution, route) {
        Ok(lock) => {
            for path in &lock.recovered {
                eprintln!(
                    "{} removed {}, left by an interrupted write",
                    cli::style::prefix(),
                    path.display()
                );
            }
            for line in &lock.recovery {
                eprintln!("{} {line}", cli::style::prefix());
            }
            Ok(lock)
        }
        Err(error) => {
            eprintln!("{} {error}", cli::style::prefix());
            Err(FAILED)
        }
    }
}

/// What an executor reported about taking the workspace lock, as the lines `take_lock` prints.
fn print_notices(notices: &commands::Notices) {
    for line in notices.lines() {
        eprintln!("{} {line}", cli::style::prefix());
    }
}

fn run(command: impl FnOnce(&Solution) -> u8) -> u8 {
    let here = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match Solution::discover(&here) {
        Ok(solution) => command(&solution),
        Err(e) => {
            eprintln!("{} {e}", cli::style::prefix());
            FAILED
        }
    }
}

fn projects(solution: &Solution) -> u8 {
    let order = match solution.deploy_order() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{} {e}", cli::style::prefix());
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
