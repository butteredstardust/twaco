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

use twaco::core::config::Solution;
use twaco::core::{adopt, backup, baseline, newblock, relocate, retemplate, catalog, status, workflow, lock, config_table, db, datatable_copy, deploy, entity_carry, entity_delete, push, profile, rename, server, types, workspace};

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
                eprintln!("twaco: {} exists and is never overwritten; remove it first to start again", target.display());
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
                println!("wrote {} with {} project(s); `twaco doctor` checks the rest", target.display(), solution.projects.len());
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
        let failed = items.iter().any(|i| i.health == twaco::core::doctor::Health::Fail);
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
        let reloaded;
        let solution = if route == "new building-block" && parsed.has("--apply") {
            reloaded = match Solution::load(&solution.root.join(twaco::core::config::CONFIG_FILE)) {
                Ok(solution) => solution,
                Err(error) => {
                    eprintln!("twaco: {error}");
                    return FAILED;
                }
            };
            &reloaded
        } else {
            solution
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
            "package" => package_cmd(solution, &parsed),
            "export" => export_cmd(solution, &parsed),
            "import" => import_cmd(solution, &parsed),
            "logs" => logs_cmd(solution, &parsed),
            "adopt" => adopt_cmd(solution, &parsed),
            "rename entity" | "rename prefix" | "rename field" | "rename service" | "rename param" | "rename table" | "rename property" => rename_cmd(solution, route, &parsed),
            "move service" | "move property" | "copy service" | "copy property" => relocate_cmd(solution, route, &parsed),
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
        let closed = ["(os error 32)", "(os error 232)", "(os error 109)"].iter().any(|code| message.contains(code));
        if message.starts_with("failed printing to stdout") && closed {
            std::process::exit(141);
        }
        default(info);
    }));
}

/// One command's arguments, after validation.
struct Args {
    names: Vec<String>,
    flags: Vec<String>,
    /// `--project <name>`, narrowing to one project of the solution.
    project: Option<String>,
    /// Server profile name. It is only resolved by server commands.
    profile: Option<String>,
    /// Raw entity GET destination.
    out: Option<PathBuf>,
    /// Repeated deploy entity selectors.
    only: Vec<String>,
    /// Deploy project selectors parsed from comma-separated values.
    only_projects: Vec<String>,
    /// Per-request timeout for an opaque service call.
    timeout: Option<Duration>,
    /// Where `config-table --backup` writes, and what `--restore` reads.
    backup: Option<PathBuf>,
    restore: Option<PathBuf>,
    /// `adopt --entity`: name fragments, repeatable.
    entity_filters: Vec<String>,
    /// Flags that take one value and need no parsing here, such as `logs --since`.
    values: std::collections::BTreeMap<String, String>,
}

/// Flags whose value is kept as text in `Args::values`, for the command to read.
const VALUE_FLAGS: &[&str] =
    &[
        "--since", "--from", "--to", "--level", "--grep", "--regex", "--user", "--thread", "--origin", "--limit", "--sublogger",
        "--version", "--section", "--member", "--repository", "--path", "--collection", "--tags", "--zip", "--search",
        "--thing", "--max-rows", "-q", "--sql-dir", "--map", "--as", "--add-shapes", "--remove-shapes", "--type", "--display-name", "--description", "--parent", "--root", "--base-extension",
    ];

impl Args {
    /// Split arguments into names and flags, refusing anything the command does not accept.
    fn parse(args: &[String], known: &[&str]) -> Result<Args, String> {
        let mut names = Vec::new();
        let mut flags = Vec::new();
        let mut project = None;
        let mut profile = None;
        let mut out = None;
        let mut only = Vec::new();
        let mut only_projects = Vec::new();
        let mut timeout = None;
        let mut backup = None;
        let mut restore = None;
        let mut entity_filters = Vec::new();
        let mut values = std::collections::BTreeMap::new();
        let mut rest = args.iter();
        while let Some(arg) = rest.next() {
            if let Some(flag) = arg.strip_prefix("--").map(|_| arg.as_str()).or_else(|| (arg == "-q").then_some("-q")) {
                if !known.contains(&flag) {
                    return Err(format!(
                        "`{flag}` is not a flag this command takes ({})",
                        if known.is_empty() { "it takes none".to_string() } else { known.join(", ") }
                    ));
                }
                match flag {
                    "--project" => {
                        project = Some(
                            rest.next().cloned().ok_or_else(|| "--project needs a name".to_string())?,
                        );
                    }
                    "--profile" => {
                        profile = Some(
                            rest.next().cloned().ok_or_else(|| "--profile needs a name".to_string())?,
                        );
                    }
                    "--out" => {
                        out = Some(PathBuf::from(
                            rest.next().cloned().ok_or_else(|| "--out needs a path".to_string())?,
                        ));
                    }
                    "--only" => only.push(
                        rest.next().cloned().ok_or_else(|| "--only needs an entity name".to_string())?,
                    ),
                    "--only-projects" => {
                        let value = rest
                            .next()
                            .cloned()
                            .ok_or_else(|| "--only-projects needs a comma-separated list".to_string())?;
                        let selected: Vec<String> = value
                            .split(',')
                            .map(str::trim)
                            .filter(|name| !name.is_empty())
                            .map(str::to_string)
                            .collect();
                        if selected.is_empty() {
                            return Err("--only-projects needs at least one project".to_string());
                        }
                        only_projects.extend(selected);
                    }
                    "--timeout" => {
                        let value = rest
                            .next()
                            .ok_or_else(|| "--timeout needs a positive number of seconds".to_string())?;
                        let seconds = value.parse::<u64>().map_err(|_| {
                            "--timeout needs a positive whole number of seconds".to_string()
                        })?;
                        if seconds == 0 {
                            return Err("--timeout needs a positive whole number of seconds".to_string());
                        }
                        timeout = Some(Duration::from_secs(seconds));
                    }
                    "--entity" => entity_filters.push(
                        rest.next().cloned().ok_or_else(|| "--entity needs a name fragment".to_string())?,
                    ),
                    "--backup" => {
                        backup = Some(PathBuf::from(
                            rest.next().cloned().ok_or_else(|| "--backup needs a file".to_string())?,
                        ));
                    }
                    "--restore" => {
                        restore = Some(PathBuf::from(
                            rest.next().cloned().ok_or_else(|| "--restore needs a file".to_string())?,
                        ));
                    }
                    valued if VALUE_FLAGS.contains(&valued) => {
                        let value = rest.next().cloned().ok_or_else(|| format!("{valued} needs a value"))?;
                        if values.insert(valued.to_string(), value).is_some() {
                            return Err(format!("{valued} is given twice"));
                        }
                    }
                    _ => flags.push(flag.to_string()),
                }
            } else {
                names.push(arg.clone());
            }
        }
        if flags.iter().any(|f| f == "--all") && !names.is_empty() {
            return Err(format!("--all and {names:?} say different things; pass one or the other"));
        }
        Ok(Args {
            names,
            flags,
            project,
            profile,
            out,
            only,
            only_projects,
            timeout,
            backup,
            restore,
            entity_filters,
            values,
        })
    }

    fn has(&self, flag: &str) -> bool {
        self.flags.iter().any(|f| f == flag)
    }
}

fn usage() {
    eprint!("{USAGE}");
}

/// Every command and flag. `documentation/COMMANDS.md` is generated from it, and a test holds
/// it to every flag the parser accepts.
const USAGE: &str = r#"usage: twaco <command>

  projects                    the solution's projects and their deploy order
  types [--check [--json]|--platform] [--profile <name>]
                              generate declarations; --check runs TypeScript once
                              suppress one finding on the line above with
                              // @ts-ignore or // @ts-expect-error
                              [[check]] hook: command = ["twaco", "types", "--check", "--json"]
  extract <entity>|--all      entity XML -> sidecars
  sync <entity>|--all         sidecars -> entity XML
      --check                 report what would change, write nothing
      --allow-add-remove      permit a service to appear or disappear
      --relayout              rewrite scripts in the configured CDATA layout
  fmt [--check]               format service scripts
  check [--detail]            every gate, one exit code
      --live                  also parse every script on the server (fails closed)
  bundle [--backend-only]     one importable document
      --check                 report whether the bundle is current, write nothing
  deploy [--apply] [--force]  check, bundle, live-parse, conflict-check, import
      --backend-only          exclude configured UI collections
      --only <entity>         include only this entity (repeatable)
      --only-projects <a,b>   include only these projects
      --skip-checks           skip offline gates; live parse still runs
      --no-backup             with --force, do not save the server's copies first
  call <target> <service> [<json>] [--timeout <seconds>] [--detail]
                              target: Collection/Name, or an entity by name or last segment
      --with-logs             then what it wrote to ScriptLog and ApplicationLog (waits up to 3 s)
  logs <log>                  read a server log: ApplicationLog, ScriptLog, CommunicationLog,
                              ConfigurationLog, SecurityLog (newest first; times are local)
      --since <n><s|m|h|d>    how far back (default 1h); or --from <time> --to <time>
      --level <L>             L and above: TRACE, DEBUG, INFO, WARN, ERROR
      --grep <text>           entries whose message contains the text
      --regex <re>            a Java regex that must match the whole message
      --user/--thread/--origin <v>  exact matches
      --limit <n>             at most n entries (default 100); --oldest-first; --json
  logs level <log> [<LEVEL>]  read a log's level and its subloggers, or plan setting it
      --sublogger <name>      a class or package within the log
      --reset                 a sublogger (or, without --sublogger, all) back to the log's level
      --apply                 make the change; it is the whole server's, and the undo is printed
  config-table <thing> <table> one Thing's configuration table on the server
      --backup <file>         save it (never overwrites a file)
      --restore <file>        put a backup back; a plan unless --apply
      --diff                  compare with the entity XML in the repository
      --detail                every row, not only the first
  entity get <entity>         fetch raw server XML to stdout or --out <path>
      [--profile <name>]      server profile (default: default)
  db run <file.sql> [--thing <name>] [--no-transaction] [--timeout <seconds>] [--apply] [--json]
                              run one atomic SQLCommand; plan unless --apply
  db query <file.sql>|-q <sql> [--thing <name>] [--max-rows <n>] [--timeout <seconds>] [--detail] [--json]
                              run a read-only SQLQuery; summary shows columns and first 20 rows
  datatable copy <old> <new> [--map a=b,c=d] [--drop-unmapped] [--append] [--max-rows <n>] [--apply] [--json]
                              copy a DataTable's rows into the one that replaced it; plan unless --apply
      --map a=b,...           source field -> target field (same names and the rename ledger match otherwise)
      --drop-unmapped         leave behind source fields that have no target field
      --append                allow a target that already has rows
  db clean [--apply] [--json] delete temporary ZZ.Twaco.Sql.* Things an interrupted db run left; plan unless --apply
  entity status [<entity>|--all] [--detail] [--record]
      --record                 record matching working/server hashes as baseline
  rename entity <old> <new> [--apply] [--text] [--detail] [--json] [--skip-checks]
                              rename exactly one entity; plan unless --apply
      --sql | --sql-dir <dir> | --no-sql   the database half, as for rename field
  rename prefix <old> <new> [--apply] [--text] [--detail] [--json] [--skip-checks]
                              rename a project/building-block prefix; --text includes other files
      --sql | --sql-dir <dir> | --no-sql   the database half, as for rename field
  rename field <datashape> <old> <new> [--apply] [--detail] [--json] [--skip-checks]
                              rename a DataShape field and its configuration tables
      --text                  refused: a field rename has no text pass
      --sql | --sql-dir <dir> | --no-sql
                              entity, prefix and field: a rename that touches DBConnection tables is
                              refused until you choose: --sql writes the migration script (default
                              folder sql/, run it before the import), --no-sql says the tables are unused
  rename service <entity> <old> <new> [--apply] [--detail] [--json] [--skip-checks]
                              rename a declared service, its overrides and repository callers
      --text                  refused: a service rename has no outside-text pass
  rename param <entity> <service> <old> <new> [--apply] [--detail] [--json] [--skip-checks]
                              rename one declared service input and repository callers
      --text                  refused: a parameter rename has no outside-text pass
  rename table <entity> <old> <new> [--apply] [--detail] [--json] [--skip-checks]
                              rename a declared configuration table and inherited instances
      --text                  refused: a table rename has no outside-text pass
  rename property <entity> <old> <new> [--apply] [--detail] [--json] [--skip-checks]
                              rename a property of a Thing, template or shape, its overrides and readers
      --text                  refused: a property rename has no text pass
  mcp                         serve the tools over MCP on stdio (TWACO_ROOT: the solution)
  doctor [--profile <name>]   what resolved, what is reachable, what is missing
  export entity <Coll/Name> --out <file>   one entity's XML, from the server's Exporter
  export collection <Coll> [--project P] --out <file>   a collection, or one project's part of it
  export project <P> --out <file>  everything of a project, as one XML
      --force                 replace the --out file if it exists
  export source-control --repository R --path <p> [--project P] [--collection C] [--tags t]
      [--zip <name>] [--with-dependents]  the source-control layout into a repository; --apply sends it
  package bundle [--project P] [--backend-only|--frontend-only] --out <file>
                              the repository as one importable XML, offline
  package source-control [--project P] --out <file.zip>  the <Project>/<Collection>/<Name>.xml layout
  package extension [--project P] [--editable] --out <file.zip>  a DPM-style extension: one project's,
                              or the solution's as a zip of its projects' ([package] in twaco.toml)
      --force                 replace the --out file if it exists
  import <file.xml|.zip>      import an export into the server: a plan of what it adds and
                              replaces unless --apply
  import source-control --repository R --path <p>  a source-control tree from a repository;
                              the plan is the server's diff; --apply imports, then diffs again
      --detail                every entity of the plan, not only the counts
      --overwrite-properties --overwrite-tables  replace the server's property values and
                              configuration table rows (kept by default, as in Composer)
  settings                    the server's subsystems: running, and how many settings tables
  settings <Subsystem> [<Table>]  every setting of one, with its value and description
  settings --search <text>    settings whose name or description contains the text
                              (read-only; PASSWORD values are never shown); --json
  catalog [<entity>] [--project P] [--search <text>] [--json]
                              offline services, signatures, origins and descriptions
  ext list [--json]           the server's extension packages
  ext show <package>          one package: its extensions, and which are in use
  ext import <zip>            validate a package on the server; --apply installs it
  ext remove <package>        plan removing a package (refused while in use); --apply removes it
  repo list                   the server's file repositories
  repo ls <repo> [<path>]     a folder's folders and files; --recursive for all below it; --json
  repo get <repo> <path>      a file to stdout, or --out <file> (never overwritten without --force)
  repo status <repo>          filerepository/<repo>/ against the server: same, differs,
                              local-only, remote-only (sizes, then SHA-256); --json
  repo put <repo> <file> <path>  upload a file; --overwrite to replace one
  repo mkdir <repo> <path>    create a folder
  repo rm <repo> <path>       delete a file, or an empty folder; --recursive for one with content
  repo mv <repo> <from> <to>  move a file; --overwrite to replace one
  repo push <repo>            upload what filerepository/<repo>/ has that the server lacks
  repo pull <repo>            download what the server has into filerepository/<repo>/
                              (neither deletes; a file differing on both sides needs --overwrite)
      --apply                 make the change: put, mkdir, rm and mv are plans without it
  guide                       the knowledge topics: twaco's workflow, the platform's verified quirks,
                              the service-code reference, and the solution's own markdown
  guide <topic> [--section <heading>]  read one (a long one gives its outline)
  guide --search <words>      the sections that best match; --limit <n>; --json
  help search <words>         search the ThingWorx Platform help (the server's version)
  help page <page>            read a help page as Markdown; --section <heading>
      --version <10.1>        another release; --limit <n>; --refresh; --json
      --profile <name>        the server whose version to read (default: default)
  javadoc search <name>       search ThingWorx Platform Java API classes and members
      --limit <n>; --refresh; --json
  javadoc class <Name|pkg.Name>  read a class as Markdown; --member <name>; --refresh; --json
  init [--write]              propose a twaco.toml from the repository's own entities; --write
                              also writes AGENTS.md and CLAUDE.md where absent
  init --agents               write only AGENTS.md and CLAUDE.md, where absent
  adopt <export.xml>          what a designer's Composer export really changes; --apply writes it in
      --entity <name>         only this entity; --detail; --json; --fail-on-revert
  entity push <entity>        import one entity, refusing if the server changed
      --apply                 actually push (without it, report what would happen)
      --force                 push over a server-side change; the server's copy is saved first
      --no-backup             with --force, do not save the server's copy first
  entity delete <entity>...   plan guarded server deletion; Collection/Name or a bare server name
      --renamed               also delete undeleted old entity/prefix names from .twaco/renames.json
      --allow-repository-defined  accept deletion of an entity the repository still defines
      --allow-outside-dependents  accept structural dependents outside this delete set
      --allow-file-repository-data-loss  accept deletion of a FileRepository Thing and its files
      --force                 deprecated: means the first two acknowledgements, never FileRepository data loss
      --apply                 delete, confirm each entity is absent, and mark ledger entries
      --no-backup             do not save the server's copies under .twaco/backups first
      --json                  entities include refusal messages and parallel refusal_codes when refused
  move service <from> <to> <name> [--as <new>] [--leave-delegate] [--apply] [--detail] [--json]
                              lift a service out of one Thing, template or shape and put it on another
      --as <new>              give it a new name on the target
      --leave-delegate        keep the service on the source, calling the moved one (a Thing target)
  move property <from> <to> <name> [--as <new>] [--apply] [--detail] [--json]
                              the same for a property definition
  copy service <from> <to> <name> [--as <new>] [--apply] [--detail] [--json]
                              as move, and the source keeps its service
  copy property <from> <to> <name> [--as <new>] [--apply] [--detail] [--json]
  new building-block <name> [--type standard|abstract|implementation] [--display-name <text>] [--description <text>]
      [--parent <block>] [--model-logic] [--no-management-shape] [--root <dir>] [--base-extension PTC.Base:<version>]
      [--apply] [--json]
                              create a building block: its project, entry point, manager, groups and
                              organization as files, and its project in twaco.toml; plan unless --apply
      --type                  standard (own manager, default), abstract (no manager Thing) or implementation
      --parent <block>        the abstract block an implementation implements
      --model-logic           also a ModelLogic_TS shape
      --no-management-shape   an implementation without its own Management_TS
      --root <dir>            the project's folder (default: the block's name)
      --base-extension        the PTC.Base extension version to depend on (default: what another project declares)
  retemplate <entity> [--to <template>] [--add-shapes a,b] [--remove-shapes a,b] [--accept-loss] [--apply] [--detail] [--json]
                              change a Thing's template (or a template's base) or its implemented shapes;
                              the plan lists what it and everything inheriting it gains and loses
      --to <template>         the new thingTemplate / baseThingTemplate
      --add-shapes a,b        implement these shapes too
      --remove-shapes a,b     stop implementing these shapes
      --accept-loss           go ahead although stored values or references would lose their definition
  entity restore [<set> [<entity>...]]  list backup sets, or plan importing one back
      --apply                 import the set's entities, confirming each on the server
      --json                  {sets|plan|applied, ...}
  entity carry <old> <new>... copy run-time, design-time and visibility permissions of renamed entities
                              (pairs written Collection/Old Collection/New); principals follow the ledger
      --renamed               also every entity the rename ledger has not yet carried or deleted
      --apply                 write the differing permissions, read each back, mark the ledger
      --detail                also ask the platform for its own difference count
      --json                  {plan|applied, entities:[collection, old, new, status, kinds, error]}

  --project <name>            narrow to one project of the solution
  --version

exit: 0 done, 1 a --check found work, 2 failed
"#;

/// Execute exactly the named opaque service. There is no dry run because twaco cannot infer
/// whether an arbitrary ThingWorx service writes. The MCP surface must decide separately
/// whether and how to expose this capability.
/// Read, back up, restore or diff one Thing's configuration table on the server.
///
/// `--restore` is a plan unless `--apply`, as every command that writes to a server is. It
/// refuses a backup made from another Thing or table, and reads the table back afterwards.
fn config_table(solution: &Solution, args: &Args) -> u8 {
    if args.names.len() != 2 {
        eprintln!("twaco: config-table needs <thing> <table>");
        return FAILED;
    }
    let modes = [args.backup.is_some(), args.restore.is_some(), args.has("--diff")];
    if modes.iter().filter(|m| **m).count() > 1 {
        eprintln!("twaco: --backup, --restore and --diff are separate actions; pass one");
        return FAILED;
    }
    if args.has("--apply") && args.restore.is_none() {
        eprintln!("twaco: --apply only means something with --restore");
        return FAILED;
    }
    // A Thing in the solution may be named by its last segment, as elsewhere. One that is not in
    // the solution is taken as given, except by --diff, which needs the repository's copy.
    let found = workspace::discover(solution).entities;
    let (thing, entity_file) = match workspace::resolve(&found, &args.names[0]) {
        Ok(entity) if entity.info.collection == "Things" => (entity.info.name.clone(), Some(entity.path.clone())),
        Ok(entity) => {
            eprintln!("twaco: {} is a {}, and only a Thing has configuration tables here", entity.info.name, entity.info.collection);
            return FAILED;
        }
        Err(workspace::WorkspaceError::UnknownEntity { .. }) if !args.has("--diff") => (args.names[0].clone(), None),
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let table = &args.names[1];
    let label = format!("{thing}.{table}");
    let profile_name = args.profile.as_deref().unwrap_or("default");
    let profile = match profile::load(&solution.root, profile_name) {
        Ok(profile) => profile,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let client = server::Client::new(profile);

    if let Some(path) = &args.restore {
        let saved = match config_table::read_backup(path, &thing, table) {
            Ok(saved) => saved,
            Err(error) => {
                eprintln!("twaco: {error}");
                return FAILED;
            }
        };
        let apply = args.has("--apply");
        return match config_table::restore(&client, &thing, table, &saved, apply) {
            Ok(plan) if plan.writes == 0 && plan.deletes.is_empty() => {
                println!("{label}: already matches the backup; nothing to restore");
                OK
            }
            Ok(plan) => {
                let (verb, removal) = if apply { ("restored", "removed") } else { ("would restore", "remove") };
                println!(
                    "{label}: {verb} {} row(s) and {removal} {} added since the backup{}",
                    plan.writes,
                    plan.deletes.len(),
                    if plan.deletes.is_empty() { String::new() } else { format!(" ({})", plan.deletes.join(", ")) }
                );
                if apply {
                    println!("read back and matching the backup");
                } else {
                    println!("dry run: nothing was written; pass --apply to restore");
                }
                OK
            }
            Err(error) => {
                eprintln!("twaco: {label}: {error}");
                FAILED
            }
        };
    }

    let live = match config_table::fetch(&client, &thing, table) {
        Ok(live) => live,
        Err(error) => {
            eprintln!("twaco: {label}: {error}");
            return FAILED;
        }
    };
    if let Some(path) = &args.backup {
        return match config_table::write_backup(path, &thing, table, &live) {
            Ok(()) => {
                println!("backed up {} row(s) of {label} to {}", live.rows.len(), path.display());
                OK
            }
            Err(error) => {
                eprintln!("twaco: {error}");
                FAILED
            }
        };
    }
    if args.has("--diff") {
        let path = entity_file.expect("--diff resolved the Thing in the solution");
        let src = match std::fs::read(&path) {
            Ok(src) => src,
            Err(error) => {
                eprintln!("twaco: {}: {error}", path.display());
                return FAILED;
            }
        };
        let repository = match config_table::repository_rows(&src, table) {
            Ok(rows) => rows,
            Err(error) => {
                eprintln!("twaco: {}: {error}", path.display());
                return FAILED;
            }
        };
        let key = config_table::primary_key(&live.data_shape);
        let found = config_table::differences("server", &live.rows, "source control", &repository, &key);
        if found.is_empty() {
            println!("{label}: {} row(s), identical to source control", live.rows.len());
            return OK;
        }
        println!("{label}: {} difference(s) from source control", found.len());
        for line in found {
            println!("  {line}");
        }
        return DRIFT;
    }
    let key = config_table::primary_key(&live.data_shape);
    println!(
        "{label}: {} row(s); primary key {}",
        live.rows.len(),
        if key.is_empty() { "none".to_string() } else { key.join(", ") }
    );
    if args.has("--detail") {
        println!("{}", serde_json::to_string_pretty(&live.rows).expect("JSON values serialise"));
    } else if let Some(first) = live.rows.first() {
        println!("first row: {}", serde_json::to_string(first).expect("JSON values serialise"));
    }
    OK
}

/// Compare a designer's export with the solution: what it would revert, and what it would change.
///
/// Read-only. Summary by default; `--detail` lists every differing node, and
/// `--json` gives the same summary as JSON.
fn adopt_cmd(solution: &Solution, args: &Args) -> u8 {
    if args.names.len() != 1 {
        eprintln!("twaco: adopt needs the path of one <Entities> export");
        return FAILED;
    }
    let export = PathBuf::from(&args.names[0]);
    let report = match adopt::compare(solution, &export, &args.entity_filters) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let reverts = report.reverts().count();

    if args.has("--json") {
        let services: Vec<serde_json::Value> = report
            .services
            .iter()
            .map(|s| serde_json::json!({ "entity": s.entity, "service": s.service, "generated": s.generated }))
            .collect();
        let changed: serde_json::Map<String, serde_json::Value> = report
            .with_status(adopt::Status::Changed)
            .map(|e| (e.entity.path(), serde_json::json!(e.differences.len())))
            .collect();
        let summary = serde_json::json!({
            "services": services,
            "new": report.with_status(adopt::Status::New).map(|e| e.entity.path()).collect::<Vec<_>>(),
            "absent": report.absent.iter().map(adopt::EntityRef::path).collect::<Vec<_>>(),
            "changed": changed,
            "identical": report.with_status(adopt::Status::Identical).count(),
        });
        println!("{}", serde_json::to_string_pretty(&summary).expect("JSON values serialise"));
    } else {
        print_adopt_report(solution, &report, args.has("--detail"));
    }
    if args.has("--apply") {
        match adopt::apply(solution, &export, &report) {
            Ok(outcome) => {
                println!("\n=== applied ({}) ===", outcome.lines.len());
                for line in &outcome.lines {
                    println!("  {line}");
                }
                print_types_refresh(&outcome.types);
                println!("\nRun `twaco sync --all` to fold the sidecars into the entity XML, then `twaco check`.");
                if reverts > 0 {
                    println!("The {reverts} service difference(s) above were not applied; they need a person.");
                }
            }
            Err(error) => {
                eprintln!("twaco: {error}");
                return FAILED;
            }
        }
    }
    if args.has("--fail-on-revert") && reverts > 0 {
        DRIFT
    } else {
        OK
    }
}

fn rename_cmd(solution: &Solution, route: &str, args: &Args) -> u8 {
    let word = route.strip_prefix("rename ").unwrap_or(route);
    let kind = rename::Kind::from_word(word).expect("the route names a rename kind");
    let mut request = match rename::Request::from_names(kind, &args.names) {
        Ok(request) => request,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    request.apply = args.has("--apply");
    request.include_outside = args.has("--text");
    request.skip_checks = args.has("--skip-checks");
    request.database = rename::DatabaseFlags {
        sql: args.has("--sql"),
        no_sql: args.has("--no-sql"),
        dir: args.values.get("--sql-dir").cloned(),
    };
    let date = jiff::Zoned::now().strftime("%Y-%m-%d").to_string();
    let (spec, options) = match request.build(&solution.root, &date) {
        Ok(built) => built,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let outcome = match rename::run(solution, &spec, &options) {
        Ok(outcome) => outcome,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    if args.has("--json") {
        print_rename_json(solution, &outcome, options.include_outside);
    } else {
        print_rename_outcome(solution, &outcome, options.include_outside, args.has("--detail"));
    }
    let verification_failed = outcome.verification.as_ref().is_some_and(|verification| {
        !verification.sync_problems.is_empty() || !verification.blocking_gates.is_empty()
    });
    if verification_failed {
        eprintln!("twaco: the rename is applied, but verification found problems; fix the sync entities and blocking gates listed above");
        FAILED
    } else {
        OK
    }
}

fn print_rename_outcome(solution: &Solution, outcome: &rename::Outcome, include_outside: bool, detail: bool) {
    let plan = &outcome.plan;
    let state = if outcome.applied.is_some() {
        "applied"
    } else {
        "a plan, nothing was written (pass --apply)"
    };
    println!("{}: {state}", plan.headline());
    for (label, text) in plan.summary_rows(include_outside, detail) {
        println!("  {label:<14} {text}");
    }
    let counts = plan.counts();
    let review = [counts.entity, counts.sidecar, counts.config, counts.outside, counts.mashup]
        .iter().map(|count| count.review).sum::<usize>();
    println!("  review         {review} occurrences left for a person");
    if !plan.named.is_empty() {
        println!(
            "  file names     {} file(s) are named for the old name and were not renamed",
            plan.named.len()
        );
    }
    if !plan.skipped.is_empty() {
        println!("  skipped        {} files that are not text", plan.skipped.len());
    }
    if let Some(script) = &outcome.sql {
        let verb = if outcome.applied.is_some() { "written to" } else { "would be written to" };
        println!("  database       a migration {verb} {}", display_relative(solution, &script.path));
    }

    if detail {
        if let Some(script) = &outcome.sql {
            println!("\n{}", script.text);
        }
        for change in plan.changes.iter().chain(&plan.outside) {
            for finding in &change.findings {
                println!("{}:{}  {}", display_relative(solution, &change.path), finding.line, finding.excerpt);
            }
        }
        for path in &plan.named {
            println!("{}", display_relative(solution, path));
        }
    } else if review > 0 {
        let findings = plan.changes.iter().chain(&plan.outside).flat_map(|change| {
            change.findings.iter().filter(|finding| finding.tier == twaco::core::refs::Tier::Review)
                .map(move |finding| (change, finding))
        }).collect::<Vec<_>>();
        for (change, finding) in findings.iter().take(10) {
            println!("{}:{}  {}", display_relative(solution, &change.path), finding.line, finding.excerpt);
        }
        if findings.len() > 10 {
            println!("and {} more", findings.len() - 10);
        }
    }

    if let Some(verification) = &outcome.verification {
        println!("\nverified");
        if verification.sync_problems.is_empty() {
            println!("  sync: in step");
        } else {
            for entity in &verification.sync_problems {
                println!("  sync: {entity} needs attention");
            }
        }
        if verification.blocking_gates.is_empty() {
            println!("  check: all gates pass");
        } else {
            for gate in &verification.blocking_gates {
                println!("  check: {gate} fails");
            }
        }
    }
    if !outcome.follow_up.is_empty() {
        println!("\nNot carried over by a rename:");
        for item in &outcome.follow_up {
            println!("  - {item}");
        }
    }
    if matches!(plan.spec.kind, rename::Kind::Field | rename::Kind::Service | rename::Kind::Param | rename::Kind::Table | rename::Kind::Property) {
        println!("Next: twaco check; commit; twaco deploy --apply.");
    } else {
        println!("Next: twaco check; commit; twaco deploy --apply; then delete the old entities from each server.");
    }
}

fn print_rename_json(solution: &Solution, outcome: &rename::Outcome, include_outside: bool) {
    let value = rename::summary_json(solution, outcome, include_outside, usize::MAX);
    println!("{}", serde_json::to_string_pretty(&value).expect("rename result serialises"));
}

fn print_adopt_report(solution: &Solution, report: &adopt::Report, detail: bool) {
    println!("=== services the export would change ===");
    if report.services.is_empty() {
        println!("  none: every service in the export matches the repository");
    }
    for service in &report.services {
        let mut notes = Vec::new();
        if service.generated {
            notes.push("generated, repository is authoritative");
        }
        if !service.sidecar {
            notes.push("compared against entity XML, no sidecar");
        }
        let suffix = if notes.is_empty() { String::new() } else { format!("  ({})", notes.join("; ")) };
        println!("  {}.{}{suffix}", service.entity, service.service);
        println!("      {}", display_relative(solution, &service.source));
    }
    let reverts = report.reverts().count();
    if reverts > 0 {
        println!("\n  {reverts} service(s) differ and are not marked generated. Each is either the");
        println!("  designer's change to adopt or one of ours they never had: read the diff before");
        println!("  importing, and redeploy the entity afterwards.");
    }
    if !report.unmatched_services.is_empty() {
        let shown: Vec<&str> = report.unmatched_services.iter().take(5).map(String::as_str).collect();
        println!(
            "\n  {} exported service(s) have no counterpart here: {}",
            report.unmatched_services.len(),
            shown.join(", ")
        );
    }

    let new: Vec<&adopt::EntityReport> = report.with_status(adopt::Status::New).collect();
    println!("\n=== new entities ({}) ===", new.len());
    for entry in &new {
        let project = if entry.project.is_empty() {
            " (no projectName)".to_string()
        } else if solution.project(&entry.project).is_none() {
            format!(" (project {}, not in this solution)", entry.project)
        } else {
            format!(" (project {})", entry.project)
        };
        println!("  {:16} {}{project}", entry.entity.collection, entry.entity.name);
    }

    println!("\n=== here but not in the export ({}) ===", report.absent.len());
    for entity in &report.absent {
        println!("  {:16} {}", entity.collection, entity.name);
    }
    if !report.absent.is_empty() {
        println!("  The export does not say whether these were deleted or never left their server.");
        println!("  Ask before dropping one: an import will not remove them either way.");
    }

    let changed: Vec<&adopt::EntityReport> = report.with_status(adopt::Status::Changed).collect();
    println!("\n=== changed entities ({}) ===", changed.len());
    for entry in &changed {
        let mut extra = Vec::new();
        if entry.volatile_ids > 0 {
            extra.push(format!("{} regenerated ids", entry.volatile_ids));
        }
        if entry.ignored > 0 {
            extra.push(format!("{} ignored", entry.ignored));
        }
        let suffix = if extra.is_empty() { String::new() } else { format!("  [{}]", extra.join(", ")) };
        println!(
            "  {:16} {}  ({} node(s)){suffix}",
            entry.entity.collection,
            entry.entity.name,
            entry.differences.len()
        );
        if detail {
            let side = |value: &Option<String>| match value {
                None => "<absent>".to_string(),
                Some(text) if text.chars().count() > 160 => format!("{}...", text.chars().take(160).collect::<String>()),
                Some(text) => text.clone(),
            };
            for difference in &entry.differences {
                println!("    {}", difference.path);
                println!("      export: {}", side(&difference.export));
                println!("      repo  : {}", side(&difference.repo));
            }
        }
    }

    let identical = report.with_status(adopt::Status::Identical).count();
    let ignored: usize = report.entities.iter().map(|e| e.ignored).sum();
    let volatile: usize = report.entities.iter().map(|e| e.volatile_ids).sum();
    println!("\n=== identical: {identical} entities ===");
    println!("    {volatile} regenerated binding/event ids and {ignored} configured-ignore node(s) were not counted as changes");
    if !detail && !changed.is_empty() {
        println!("run with --detail to see each differing node");
    }
}

fn display_relative(solution: &Solution, path: &Path) -> String {
    path.strip_prefix(&solution.root).unwrap_or(path).display().to_string()
}

fn call(solution: &Solution, args: &Args) -> u8 {
    if !(2..=3).contains(&args.names.len()) {
        eprintln!("twaco: call needs <target> <service> and an optional JSON object");
        return FAILED;
    }
    let parameters = match args.names.get(2) {
        Some(text) => match serde_json::from_str::<serde_json::Value>(text) {
            Ok(value) if value.is_object() => value,
            Ok(_) => {
                eprintln!("twaco: call parameters must be a JSON object");
                return FAILED;
            }
            Err(error) => {
                eprintln!("twaco: call parameters are not valid JSON: {error}");
                return FAILED;
            }
        },
        None => serde_json::json!({}),
    };
    let profile_name = args.profile.as_deref().unwrap_or("default");
    let profile = match profile::load(&solution.root, profile_name) {
        Ok(profile) => profile,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let target = match workspace::call_target(&workspace::discover(solution).entities, &args.names[0]) {
        Ok(target) => {
            // Said aloud: a platform Thing with the same short name is reached as Things/<Name>.
            if target.to_string() != args.names[0] {
                eprintln!("twaco: calling {target}");
            }
            target
        }
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let client = server::Client::new(profile);
    let started = twaco::core::logs::now_ms();
    let result = client.call_service(
        &target,
        &args.names[1],
        &parameters,
        args.timeout.unwrap_or(Duration::from_secs(120)),
    );
    if args.has("--with-logs") {
        // Printed first, so a failure's own lines come before its error, as they happened.
        use twaco::core::logs;
        let ended = logs::now_ms();
        match logs::during_call(&client, started, ended, logs::Wait::default(), &logs::now_ms, &std::thread::sleep) {
            Ok(entries) => {
                println!("--- logged during the call: {} entr{}", entries.len(), if entries.len() == 1 { "y" } else { "ies" });
                for (log, entry) in &entries {
                    println!("{log}: {}", logs::line(entry));
                }
                println!("---");
            }
            Err(error) => eprintln!("twaco: the call's logs could not be read: {error}"),
        }
    }
    match result {
        Ok(None) => println!("done"),
        Ok(Some(value)) if args.has("--detail") || !is_info_table(&value) => {
            println!("{}", serde_json::to_string_pretty(&value).expect("JSON value serialises"));
        }
        Ok(Some(value)) => print_info_table_summary(&value),
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    }
    OK
}

/// Read one of the server's logs. Read-only, so it takes no lock.
fn logs_cmd(solution: &Solution, args: &Args) -> u8 {
    use twaco::core::logs;
    if args.names.first().map(String::as_str) == Some("level") {
        return log_level_cmd(solution, args);
    }
    let value = |flag: &str| args.values.get(flag).map(String::as_str);
    for only_for_levels in ["--sublogger", "--reset", "--apply"] {
        if args.has(only_for_levels) || args.values.contains_key(only_for_levels) {
            eprintln!("twaco: logs: {only_for_levels} belongs to `twaco logs level`");
            return FAILED;
        }
    }
    let built = (|| -> Result<logs::Query, String> {
        let [log] = args.names.as_slice() else {
            return Err(format!("logs needs one log name: {}", logs::LOGS.join(", ")));
        };
        let now = logs::now_ms();
        let (from_ms, to_ms) = match (value("--since"), value("--from"), value("--to")) {
            (Some(_), Some(_), _) | (Some(_), _, Some(_)) => {
                return Err("--since is a window ending now; give it, or --from and --to, not both".to_string())
            }
            (since, None, None) => (now - logs::parse_since(since.unwrap_or("1h")).map_err(|e| e.to_string())?, now),
            (None, from, to) => {
                let to_ms = logs::parse_time(to.unwrap_or("now"), now).map_err(|e| e.to_string())?;
                let from_ms = match from {
                    Some(from) => logs::parse_time(from, now).map_err(|e| e.to_string())?,
                    None => to_ms - 3_600_000,
                };
                (from_ms, to_ms)
            }
        };
        let search = match (value("--grep"), value("--regex")) {
            (Some(_), Some(_)) => return Err("--grep and --regex are two ways to search; give one".to_string()),
            (Some(text), None) => Some(logs::Search::Grep(text.to_string())),
            (None, Some(expression)) => Some(logs::Search::Regex(expression.to_string())),
            (None, None) => None,
        };
        let limit = match value("--limit") {
            Some(text) => text.parse::<u64>().map_err(|_| format!("--limit needs a whole number, not {text:?}"))?,
            None => 100,
        };
        Ok(logs::Query {
            log: log.clone(),
            from_ms,
            to_ms,
            level: value("--level").map(logs::level).transpose().map_err(|e| e.to_string())?,
            search,
            user: value("--user").map(str::to_string),
            thread: value("--thread").map(str::to_string),
            origin: value("--origin").map(str::to_string),
            limit,
            oldest_first: args.has("--oldest-first"),
        })
    })();
    let query = match built {
        Ok(query) => query,
        Err(why) => {
            eprintln!("twaco: logs: {why}");
            return FAILED;
        }
    };
    let profile_name = args.profile.as_deref().unwrap_or("default");
    let client = match profile::load(&solution.root, profile_name) {
        Ok(profile) => server::Client::new(profile),
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let outcome = match logs::query(&client, &query) {
        Ok(outcome) => outcome,
        Err(error) => {
            eprintln!("twaco: logs: {error}");
            return FAILED;
        }
    };
    for entry in &outcome.entries {
        if args.has("--json") {
            println!("{}", logs::entry_json(entry));
        } else {
            println!("{}", logs::line(entry));
        }
    }
    let count = outcome.entries.len();
    let mut summary = format!(
        "{count} {} from {}, {} to {}",
        if count == 1 { "entry" } else { "entries" },
        query.log,
        logs::local(outcome.from_ms),
        logs::local(outcome.to_ms)
    );
    if outcome.widened {
        summary.push_str(" (widened to the platform's 5 s minimum)");
    }
    if outcome.truncated {
        summary.push_str(" (limit reached; there may be more)");
    }
    if args.has("--json") {
        eprintln!("{summary}");
    } else {
        println!("{summary}");
    }
    OK
}

/// `twaco logs level <log> [<LEVEL>] [--sublogger S] [--reset] [--apply]`. Reading is the default;
/// a change is a plan unless --apply, as for every server write. It writes no workspace file.
fn log_level_cmd(solution: &Solution, args: &Args) -> u8 {
    use twaco::core::logs;
    let request = (|| -> Result<(String, Option<logs::Change>), String> {
        let rest = &args.names[1..];
        let sublogger = args.values.get("--sublogger").cloned();
        let (log, level) = match rest {
            [log] => (log.clone(), None),
            [log, level] => (log.clone(), Some(logs::level(level).map_err(|e| e.to_string())?)),
            _ => return Err("logs level needs a log name and, to change it, a level".to_string()),
        };
        let change = match (level, args.has("--reset")) {
            (Some(_), true) => return Err("give a level or --reset, not both".to_string()),
            (Some(level), false) => Some(logs::Change::Set { level, sublogger }),
            (None, true) => Some(logs::Change::Reset { sublogger }),
            (None, false) if sublogger.is_some() => {
                return Err("--sublogger needs a level to set, or --reset".to_string())
            }
            (None, false) => None,
        };
        if change.is_none() && args.has("--apply") {
            return Err("--apply needs a level to set, or --reset".to_string());
        }
        Ok((log, change))
    })();
    let (log, change) = match request {
        Ok(request) => request,
        Err(why) => {
            eprintln!("twaco: logs level: {why}");
            return FAILED;
        }
    };
    let profile_name = args.profile.as_deref().unwrap_or("default");
    let client = match profile::load(&solution.root, profile_name) {
        Ok(profile) => server::Client::new(profile),
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let Some(change) = change else {
        return match logs::levels(&client, &log) {
            Ok(levels) => {
                println!("{log}: {}", levels.level);
                for (name, level) in &levels.subloggers {
                    println!("  {name}: {level}");
                }
                OK
            }
            Err(error) => {
                eprintln!("twaco: logs level: {error}");
                FAILED
            }
        };
    };
    let apply = args.has("--apply");
    match logs::change(&client, &log, &change, apply) {
        Ok(report) => {
            if apply {
                println!("changed {}", report.plan);
            } else {
                println!("would change {}; nothing sent (pass --apply; the level is the whole server's)", report.plan);
            }
            for undo in &report.undo {
                println!("to put it back: {undo}");
            }
            OK
        }
        Err(error) => {
            eprintln!("twaco: logs level: {error}");
            FAILED
        }
    }
}

/// `twaco help search <words> | page <page>`: the ThingWorx Platform help center, for the
/// server's own version unless told otherwise. Read-only; downloads go to the user's cache.
/// AGENTS.md and CLAUDE.md for the solution, each only where none exists.
fn write_agent_files(solution: &Solution) -> u8 {
    let projects: Vec<String> = match solution.deploy_order() {
        Ok(order) => order.iter().map(|p| p.name.clone()).collect(),
        Err(_) => solution.projects.iter().map(|p| p.name.clone()).collect(),
    };
    match twaco::core::init::write_agent_files(&solution.root, &solution.solution.name, &projects) {
        Ok((wrote, kept)) => {
            for file in &wrote {
                match file.as_str() {
                    "AGENTS.md" => println!("wrote AGENTS.md: fill in its project context; the next agent starts from it"),
                    other => println!("wrote {other}"),
                }
            }
            for file in &kept {
                println!("{file} exists and is left alone");
            }
            OK
        }
        Err(error) => {
            eprintln!("twaco: agent files: {error}");
            FAILED
        }
    }
}

/// `twaco guide [<topic> [--section <heading>]] [--search <words>]`: the knowledge an agent needs
/// besides the CLI, built in and the solution's own. Works outside a solution too.
const GUIDE_FLAGS: &[&str] = &["--section", "--search", "--limit", "--json"];
const HELP_FLAGS: &[&str] = &["--version", "--limit", "--section", "--refresh", "--json", "--profile"];
const JAVADOC_FLAGS: &[&str] = &["--member", "--limit", "--refresh", "--json"];

fn guide_cmd(args: &[String]) -> u8 {
    use twaco::core::guide;
    let parsed = match Args::parse(args, GUIDE_FLAGS) {
        Ok(parsed) => parsed,
        Err(why) => {
            eprintln!("twaco: guide: {why}");
            return FAILED;
        }
    };
    let here = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let solution = match Solution::discover(&here) {
        Ok(solution) => Some(solution),
        Err(twaco::core::config::ConfigError::NotFound { .. }) => None,
        Err(error) => {
            eprintln!("twaco: guide: {error}");
            return FAILED;
        }
    };
    let (topics, problems) = guide::topics(solution.as_ref());
    for problem in &problems {
        eprintln!("twaco: guide: {problem}");
    }
    let result: Result<(), String> = (|| {
        if let Some(query) = parsed.values.get("--search") {
            if !parsed.names.is_empty() {
                return Err("--search searches every topic; pass no topic with it".to_string());
            }
            let limit = match parsed.values.get("--limit") {
                Some(n) => n.parse::<usize>().ok().filter(|n| *n > 0).ok_or("--limit takes a positive number")?,
                None => 10,
            };
            let hits = guide::search(&topics, query, limit);
            for hit in &hits {
                if parsed.has("--json") {
                    println!(
                        "{}",
                        serde_json::json!({ "topic": hit.topic, "section": hit.heading, "matched": hit.matched, "of": hit.of, "line": hit.line })
                    );
                } else {
                    let heading = if hit.heading.is_empty() { "(introduction)" } else { hit.heading.as_str() };
                    println!("{}: {heading}  [{}/{} words]", hit.topic, hit.matched, hit.of);
                    if !hit.line.is_empty() {
                        println!("    {}", hit.line);
                    }
                }
            }
            if hits.is_empty() {
                eprintln!("nothing matches {query:?}");
            } else if !parsed.has("--json") {
                eprintln!("read one: twaco guide <topic> --section \"<heading>\"");
            }
            return Ok(());
        }
        match parsed.names.as_slice() {
            [] => {
                for topic in &topics {
                    let origin = if topic.file.is_some() { "solution" } else { "built in" };
                    println!("{:<width$}  {:<9} {}", topic.id, origin, topic.title, width = topics.iter().map(|t| t.id.len()).max().unwrap_or(0));
                }
                eprintln!("read one: twaco guide <topic>; search all: twaco guide --search <words>");
                Ok(())
            }
            [wanted] => {
                let topic = guide::find(&topics, wanted).map_err(|e| e.to_string())?;
                match guide::read(topic, parsed.values.get("--section").map(String::as_str)).map_err(|e| e.to_string())? {
                    guide::Reading::Text(text) => print!("{text}"),
                    guide::Reading::Outline { title, headings } => {
                        println!("# {title}\n");
                        for heading in &headings {
                            println!("- {heading}");
                        }
                        eprintln!("{} sections; read one: twaco guide {} --section \"<heading or part of it>\"", headings.len(), topic.id);
                    }
                }
                Ok(())
            }
            _ => Err("guide takes one topic".to_string()),
        }
    })();
    match result {
        Ok(()) => OK,
        Err(why) => {
            eprintln!("twaco: guide: {why}");
            FAILED
        }
    }
}

fn help_cmd(args: &[String]) -> u8 {
    use twaco::core::help;
    let parsed = match Args::parse(args, HELP_FLAGS) {
        Ok(parsed) => parsed,
        Err(why) => {
            eprintln!("twaco: help: {why}");
            return FAILED;
        }
    };
    let (Some(action), rest) = (parsed.names.first().map(String::as_str), parsed.names.get(1..).unwrap_or_default())
    else {
        eprintln!("twaco: help needs `search <words>` or `page <page>`");
        return FAILED;
    };
    let here = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    // No twaco.toml means no solution, and the newest help; a broken one is an error, not
    // a reason to skip the configured and the server's versions.
    let solution = match Solution::discover(&here) {
        Ok(solution) => Some(solution),
        Err(twaco::core::config::ConfigError::NotFound { .. }) => None,
        Err(error) => {
            eprintln!("twaco: help: {error}");
            return FAILED;
        }
    };
    let mut notes = Vec::new();
    let named = match action {
        "page" => match rest {
            [page] => match help::page_path(page) {
                Ok((version, _)) => version,
                Err(error) => {
                    eprintln!("twaco: help: {error}");
                    return FAILED;
                }
            },
            _ => {
                eprintln!("twaco: help page needs one page: a path such as ThingWorx/Welcome.html, or its address");
                return FAILED;
            }
        },
        _ => None,
    };
    let version = match help::choose_version(
        parsed.values.get("--version").map(String::as_str),
        named,
        solution.as_ref(),
        parsed.profile.as_deref().unwrap_or("default"),
        &mut notes,
    ) {
        Ok(version) => version,
        Err(why) => {
            eprintln!("twaco: help: {why}");
            return FAILED;
        }
    };
    for note in &notes {
        eprintln!("twaco: {note}");
    }
    let cache = match help::cache_root() {
        Ok(cache) => cache,
        Err(error) => {
            eprintln!("twaco: help: {error}");
            return FAILED;
        }
    };
    let web = help::Web::default();
    let refresh = parsed.has("--refresh");
    match action {
        "search" => {
            if rest.is_empty() {
                eprintln!("twaco: help search needs words");
                return FAILED;
            }
            let limit = match parsed.values.get("--limit").map(|n| n.parse::<usize>()) {
                None => 10,
                Some(Ok(n)) if n > 0 => n,
                Some(_) => {
                    eprintln!("twaco: help: --limit needs a positive whole number");
                    return FAILED;
                }
            };
            let index = match help::cached(&web, &cache, &version, help::INDEX_FILE, refresh)
                .and_then(|bytes| help::Index::parse(&String::from_utf8_lossy(&bytes)))
            {
                Ok(index) => index,
                Err(error) => {
                    eprintln!("twaco: help: {error}");
                    return FAILED;
                }
            };
            let found = help::search(&index, &version, &rest.join(" "), limit);
            if parsed.has("--json") {
                for hit in &found.hits {
                    println!(
                        "{}",
                        serde_json::json!({ "title": hit.page.title, "path": hit.page.path, "url": hit.url, "summary": hit.page.summary, "score": hit.score })
                    );
                }
            } else {
                for hit in &found.hits {
                    println!("{}
  {}
  {}
", hit.page.title, hit.page.path, hit.page.summary);
                }
            }
            if !found.unknown.is_empty() {
                eprintln!("twaco: the {version} help never uses: {}", found.unknown.join(", "));
            }
            eprintln!("{} of {} matching page(s) from the ThingWorx Platform {version} help", found.hits.len(), found.matched);
            OK
        }
        "page" => {
            let section = parsed.values.get("--section").map(String::as_str);
            let (_, path) = help::page_path(&rest[0]).expect("checked above");
            match help::cached(&web, &cache, &version, &path, refresh)
                .and_then(|html| help::read(&html, &version, &path, section))
            {
                Ok(read) => {
                    println!("{}", read.markdown);
                    eprintln!("\nfrom {}", read.url);
                    OK
                }
                Err(error) => {
                    eprintln!("twaco: help: {error}");
                    FAILED
                }
            }
        }
        other => {
            eprintln!("twaco: help has `search` and `page`, not {other:?}");
            FAILED
        }
    }
}

/// `twaco javadoc search <name> | class <Name>`: the public ThingWorx Java API docs.
/// Read-only and usable outside a solution; downloads go to the user's cache.
fn javadoc_cmd(args: &[String]) -> u8 {
    use twaco::core::{help, javadoc};
    let parsed = match Args::parse(args, JAVADOC_FLAGS) {
        Ok(parsed) => parsed,
        Err(why) => {
            eprintln!("twaco: javadoc: {why}");
            return FAILED;
        }
    };
    let (Some(action), rest) = (parsed.names.first().map(String::as_str), parsed.names.get(1..).unwrap_or_default())
    else {
        eprintln!("twaco: javadoc needs `search <name>` or `class <Name>`");
        return FAILED;
    };
    match action {
        "search" if rest.is_empty() => {
            eprintln!("twaco: javadoc search needs a name");
            return FAILED;
        }
        "class" if rest.len() != 1 => {
            eprintln!("twaco: javadoc class needs one class name");
            return FAILED;
        }
        "search" | "class" => {}
        other => {
            eprintln!("twaco: javadoc has `search` and `class`, not {other:?}");
            return FAILED;
        }
    }
    let cache = match javadoc::cache_root() {
        Ok(cache) => cache,
        Err(error) => {
            eprintln!("twaco: javadoc: {error}");
            return FAILED;
        }
    };
    let web = help::Web::new(javadoc::BASE);
    let refresh = parsed.has("--refresh");
    let fetch = |path: &str| javadoc::cached(&web, &cache, path, refresh);
    let types = match fetch(javadoc::TYPE_INDEX) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("twaco: javadoc: {error}");
            return FAILED;
        }
    };
    match action {
        "search" => {
            if parsed.values.contains_key("--member") {
                eprintln!("twaco: javadoc: --member is only for `class`");
                return FAILED;
            }
            let limit = match parsed.values.get("--limit").map(|n| n.parse::<usize>()) {
                None => 10,
                Some(Ok(n)) if n > 0 => n,
                Some(_) => {
                    eprintln!("twaco: javadoc: --limit needs a positive whole number");
                    return FAILED;
                }
            };
            let members = match fetch(javadoc::MEMBER_INDEX) {
                Ok(bytes) => bytes,
                Err(error) => {
                    eprintln!("twaco: javadoc: {error}");
                    return FAILED;
                }
            };
            let index = match javadoc::Index::parse(&String::from_utf8_lossy(&types), &String::from_utf8_lossy(&members)) {
                Ok(index) => index,
                Err(error) => {
                    eprintln!("twaco: javadoc: {error}");
                    return FAILED;
                }
            };
            let hits = javadoc::search(&index, &rest.join(" "), limit);
            for hit in &hits {
                if parsed.has("--json") {
                    println!("{}", serde_json::json!({
                        "kind": if hit.kind == javadoc::Kind::Class { "class" } else { "member" },
                        "package": hit.package, "class": hit.class, "label": hit.label,
                        "path": hit.path, "url": hit.url, "rank": hit.rank,
                    }));
                } else {
                    println!("{}", hit.display());
                }
            }
            eprintln!("{} result(s) from the ThingWorx Platform API {} Javadoc", hits.len(), javadoc::VERSION);
            OK
        }
        "class" => {
            let index = match javadoc::Index::parse(&String::from_utf8_lossy(&types), "memberSearchIndex = []") {
                Ok(index) => index,
                Err(error) => {
                    eprintln!("twaco: javadoc: {error}");
                    return FAILED;
                }
            };
            let class = match javadoc::find_class(&index, &rest[0]) {
                Ok(class) => class,
                Err(error) => {
                    eprintln!("twaco: javadoc: {error}");
                    return FAILED;
                }
            };
            let path = match javadoc::class_path(&class) {
                Ok(path) => path,
                Err(error) => {
                    eprintln!("twaco: javadoc: {error}");
                    return FAILED;
                }
            };
            match fetch(&path).and_then(|html| javadoc::read(&html, &class, parsed.values.get("--member").map(String::as_str))) {
                Ok(read) => {
                    if parsed.has("--json") {
                        println!("{}", serde_json::json!({
                            "version": javadoc::VERSION, "title": read.title, "path": read.path,
                            "url": read.url, "methods": read.methods, "markdown": read.markdown,
                        }));
                    } else {
                        println!("{}", read.markdown);
                        eprintln!("from {}", read.url);
                    }
                    OK
                }
                Err(error) => {
                    eprintln!("twaco: javadoc: {error}");
                    FAILED
                }
            }
        }
        _ => unreachable!("the action was checked above"),
    }
}

/// `twaco export entity | collection | project | source-control`.
fn export_cmd(solution: &Solution, args: &Args) -> u8 {
    use twaco::core::export;
    let profile_name = args.profile.as_deref().unwrap_or("default");
    let client = match profile::load(&solution.root, profile_name) {
        Ok(profile) => server::Client::new(profile),
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let value = |flag: &str| args.values.get(flag).cloned();
    let names: Vec<&str> = args.names.iter().map(String::as_str).collect();
    let result: Result<(), String> = (|| {
        let what = match names.as_slice() {
            ["entity", entity] => export::What::entity(entity).map_err(|e| e.to_string())?,
            ["collection", collection] => export::What::Collection { collection: collection.to_string(), project: args.project.clone() },
            ["project", project] => export::What::Project { project: project.to_string() },
            ["source-control"] => {
                let repository = value("--repository").ok_or("source-control needs --repository")?;
                let path = value("--path").ok_or("source-control needs --path, a folder of the repository")?;
                let filters = export::Filters {
                    project: args.project.clone(),
                    collection: value("--collection"),
                    tags: value("--tags"),
                    include_dependents: args.has("--with-dependents"),
                };
                let zip = value("--zip");
                let apply = args.has("--apply");
                let (plan, link) = export::source_control(&client, &repository, &path, &filters, zip.as_deref(), apply)
                    .map_err(|e| e.to_string())?;
                if apply {
                    println!("done: {plan}");
                    if let Some(link) = link {
                        println!("download: {link}");
                    }
                } else {
                    println!("would {plan}; nothing sent (pass --apply)");
                }
                return Ok(());
            }
            _ => return Err("export takes: entity <Coll/Name> | collection <Coll> | project <P> | source-control".to_string()),
        };
        let out = args.out.as_ref().ok_or("an export needs --out <file>")?;
        if out.exists() && !args.has("--force") {
            return Err(format!("{} exists; pass --force to replace it", out.display()));
        }
        let exported = export::export(&client, &what).map_err(|e| e.to_string())?;
        if let Some(folder) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
        }
        workspace::write_entity(out, &exported.xml).map_err(|e| e.to_string())?;
        let counts: Vec<String> = exported.counts.iter().map(|(c, n)| format!("{n} {c}")).collect();
        println!(
            "{} bytes to {}: {}",
            exported.xml.len(),
            out.display(),
            if counts.is_empty() { "no entities".to_string() } else { counts.join(", ") }
        );
        Ok(())
    })();
    match result {
        Ok(()) => OK,
        Err(why) => {
            eprintln!("twaco: export: {why}");
            FAILED
        }
    }
}

/// `twaco import <file> | import source-control`: into the server, as plans unless applied.
fn import_cmd(solution: &Solution, args: &Args) -> u8 {
    use twaco::core::imports;
    let profile_name = args.profile.as_deref().unwrap_or("default");
    let client = match profile::load(&solution.root, profile_name) {
        Ok(profile) => server::Client::new(profile),
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let apply = args.has("--apply");
    let (properties, tables) = (args.has("--overwrite-properties"), args.has("--overwrite-tables"));
    let differs_line = |d: &imports::Differs| format!("{} {}: {}", d.entity_type, d.name, d.what.join("; "));
    let names: Vec<&str> = args.names.iter().map(String::as_str).collect();
    let result: Result<(), String> = (|| match names.as_slice() {
        ["source-control"] => {
            let repository = args.values.get("--repository").ok_or("import source-control needs --repository")?;
            let path = args.values.get("--path").ok_or("import source-control needs --path")?;
            let imported = imports::import_source_control(&client, repository, path, properties, tables, apply)
                .map_err(|e| e.to_string())?;
            let (total, before, after) = (imported.total, imported.differ, imported.still_differ);
            let shown = if args.has("--detail") { usize::MAX } else { 20 };
            println!("{total} entities in {repository}:{path}; {} differ from the server", before.len());
            for d in before.iter().take(shown) {
                println!("  {}", differs_line(d));
            }
            match after {
                None => println!("nothing sent (pass --apply to import them)"),
                Some(after) => {
                    println!("imported; {} still differ{}", after.len(), if after.is_empty() { "" } else { ":" });
                    for d in after.iter().take(shown) {
                        println!("  {}", differs_line(d));
                    }
                }
            }
            Ok(())
        }
        [file] => {
            let bytes = std::fs::read(file).map_err(|e| format!("{file}: {e}"))?;
            let file_name = std::path::Path::new(file).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "import.xml".into());
            let plan = imports::import_file(&client, &file_name, &bytes, properties, tables, apply).map_err(|e| e.to_string())?;
            let shown = if args.has("--detail") { usize::MAX } else { 20 };
            for (collection, name) in plan.replaced.iter().take(shown) {
                println!("  replaces {collection}/{name}");
            }
            for (collection, name) in plan.new.iter().take(shown) {
                println!("  adds     {collection}/{name}");
            }
            println!(
                "{} {} new and {} replaced entities{}",
                if plan.applied { "imported:" } else { "would import:" },
                plan.new.len(),
                plan.replaced.len(),
                if plan.applied { "; every one is on the server" } else { "; nothing sent (pass --apply)" }
            );
            Ok(())
        }
        _ => Err("import takes: <file.xml|.zip> | source-control --repository R --path <p>".to_string()),
    })();
    match result {
        Ok(()) => OK,
        Err(why) => {
            eprintln!("twaco: import: {why}");
            FAILED
        }
    }
}

/// `twaco package bundle | source-control | extension`: the repository packaged for release,
/// offline. Never replaces an existing `--out` file without `--force`.
fn package_cmd(solution: &Solution, args: &Args) -> u8 {
    use twaco::core::package;
    let result: Result<(), String> = (|| {
        let out = args.out.as_ref().ok_or("package needs --out <file>")?;
        if out.exists() && !args.has("--force") {
            return Err(format!("{} exists; pass --force to replace it", out.display()));
        }
        let project = args.project.as_deref();
        let (bytes, summary) = match args.names.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
            ["bundle"] => {
                let part = match (args.has("--backend-only"), args.has("--frontend-only")) {
                    (true, true) => return Err("--backend-only and --frontend-only say different things".to_string()),
                    (true, false) => package::Part::Backend,
                    (false, true) => package::Part::Frontend,
                    (false, false) => package::Part::All,
                };
                let built = package::bundle(solution, project, part).map_err(|e| e.to_string())?;
                let count: usize = built.entities.values().sum();
                (built.bytes, format!("{count} entities from {} files", built.files))
            }
            ["source-control"] => {
                let (bytes, count) = package::source_control(solution, project).map_err(|e| e.to_string())?;
                (bytes, format!("{count} entities"))
            }
            ["extension"] => {
                let meta = package::Metadata::from_solution(solution);
                let editable = args.has("--editable");
                let kind = if editable { "editable" } else { "non-editable" };
                match project {
                    Some(project) => {
                        let (bytes, count) = package::extension(solution, project, editable, &meta).map_err(|e| e.to_string())?;
                        (bytes, format!("{kind} {project} {}, {count} entities", meta.version))
                    }
                    None => {
                        let (bytes, counts) = package::solution_extensions(solution, editable, &meta).map_err(|e| e.to_string())?;
                        let each: Vec<String> = counts.iter().map(|(p, n)| format!("{p} ({n})")).collect();
                        (bytes, format!("{kind} {} {}: {}", solution.solution.name, meta.version, each.join(", ")))
                    }
                }
            }
            _ => return Err("package takes: bundle | source-control | extension".to_string()),
        };
        if let Some(folder) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
        }
        workspace::write_entity(out, &bytes).map_err(|e| e.to_string())?;
        println!("{} bytes to {}: {summary}", bytes.len(), out.display());
        Ok(())
    })();
    match result {
        Ok(()) => OK,
        Err(why) => {
            eprintln!("twaco: package: {why}");
            FAILED
        }
    }
}

/// `twaco settings [<Subsystem> [<Table>]] [--search <text>]`: the server's subsystem
/// settings, read-only.
fn settings_cmd(solution: &Solution, args: &Args) -> u8 {
    use twaco::core::settings;
    let profile_name = args.profile.as_deref().unwrap_or("default");
    let client = match profile::load(&solution.root, profile_name) {
        Ok(profile) => server::Client::new(profile),
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let print_table = |subsystem: &str, table: &settings::Table| {
        println!("{subsystem}.{}", table.name);
        if table.rows.is_empty() {
            println!("  (no rows)");
        }
        for (index, row) in table.rows.iter().enumerate() {
            if table.rows.len() > 1 {
                println!("  row {}", index + 1);
            }
            for field in &table.fields {
                let value = row.get(&field.name).map(settings::shown).unwrap_or_default();
                println!("  {:<40} {:<24} {}", field.name, value, field.description);
            }
        }
    };
    let result: Result<(), String> = (|| {
        if let Some(text) = args.values.get("--search") {
            let all = settings::read_all(&client).map_err(|e| e.to_string())?;
            let found = settings::search(&all, text);
            for f in &found {
                let values: Vec<String> = f.values.iter().map(settings::shown).collect();
                if args.has("--json") {
                    println!(
                        "{}",
                        serde_json::json!({ "subsystem": f.subsystem, "table": f.table, "setting": f.field.name, "type": f.field.base_type, "values": f.values, "description": f.field.description })
                    );
                } else {
                    println!("{}.{}.{} = {}  ({})", f.subsystem, f.table, f.field.name, values.join(" | "), f.field.description);
                }
            }
            eprintln!("{} setting(s) match {text:?}", found.len());
            return Ok(());
        }
        let names = settings::Remote::subsystems(&client).map_err(|e| e.to_string())?;
        match args.names.as_slice() {
            [] => {
                let all = settings::summaries(&client).map_err(|e| e.to_string())?;
                for settings::Summary { name, running, tables } in &all {
                    if args.has("--json") {
                        println!("{}", serde_json::json!({ "subsystem": name, "running": running, "tables": tables }));
                    } else {
                        println!("{name:<34} {:<8} {}", if *running { "running" } else { "stopped" }, tables.join(", "));
                    }
                }
                eprintln!("{} subsystem(s)", all.len());
                Ok(())
            }
            [subsystem, rest @ ..] if rest.len() <= 1 => {
                let name = settings::resolve(&names, subsystem).map_err(|e| e.to_string())?;
                let read = settings::read(&client, name).map_err(|e| e.to_string())?;
                let mut shown = 0;
                for table in &read.tables {
                    if rest.first().is_some_and(|wanted| !table.name.eq_ignore_ascii_case(wanted)) {
                        continue;
                    }
                    shown += 1;
                    if args.has("--json") {
                        println!("{}", settings::table_json(&read.name, table));
                    } else {
                        print_table(&read.name, table);
                    }
                }
                if shown == 0 {
                    let tables: Vec<&str> = read.tables.iter().map(|t| t.name.as_str()).collect();
                    return Err(format!("{} has no table {:?}; it has: {}", read.name, rest.first().map(String::as_str).unwrap_or(""), tables.join(", ")));
                }
                Ok(())
            }
            _ => Err("settings takes: [<Subsystem> [<Table>]] or --search <text>".to_string()),
        }
    })();
    match result {
        Ok(()) => OK,
        Err(why) => {
            eprintln!("twaco: settings: {why}");
            FAILED
        }
    }
}

/// `twaco catalog [<entity>] [--project P] [--search <text>] [--json]`.
fn catalog_cmd(solution: &Solution, args: &Args) -> u8 {
    if args.names.len() > 1 {
        eprintln!("twaco: catalog takes at most one entity name");
        return FAILED;
    }
    let query = catalog::Query {
        project: args.project.as_deref(),
        entity: args.names.first().map(String::as_str),
        text: args.values.get("--search").map(String::as_str),
    };
    let result = match catalog::build(solution, query) {
        Ok(result) => result,
        Err(error) => {
            eprintln!("twaco: catalog: {error}");
            return FAILED;
        }
    };
    for skipped in &result.skipped {
        eprintln!("twaco: skipped {skipped}");
    }
    for entity in &result.entities {
        if !args.has("--json") {
            let mut notes = Vec::new();
            if !entity.inherits.is_empty() {
                notes.push(format!("inherits {}", entity.inherits.join(", ")));
            }
            if !entity.implemented_by.is_empty() {
                notes.push(format!("implemented by {}", entity.implemented_by.join(", ")));
            }
            let notes = if notes.is_empty() { String::new() } else { format!("  [{}]", notes.join("; ")) };
            println!("{}/{}{}", entity.collection, entity.name, notes);
        }
        for service in &entity.services {
            if args.has("--json") {
                println!("{}", serde_json::json!({
                    "collection": entity.collection,
                    "entity": entity.name,
                    "project": entity.project,
                    "inherits": entity.inherits,
                    "implemented_by": entity.implemented_by,
                    "service": service.name,
                    "parameters": service.parameters,
                    "result": service.result,
                    "description": service.description,
                    "from": service.from,
                    "has_script": service.has_script,
                }));
            } else {
                let parameters = service.parameters.iter().map(|parameter| {
                    format!("{}: {}", parameter.name, catalog_type(&parameter.base_type, parameter.data_shape.as_deref()))
                }).collect::<Vec<_>>().join(", ");
                let origin = if service.from == "own" { "[own]".to_string() } else { format!("[from {}]", service.from) };
                let description = if service.description.is_empty() {
                    String::new()
                } else {
                    format!("  {}", service.description.split_whitespace().collect::<Vec<_>>().join(" "))
                };
                println!("  {}({}) -> {}  {}{}", service.name, parameters, catalog_type(&service.result.base_type, service.result.data_shape.as_deref()), origin, description);
            }
        }
    }
    eprintln!("{} service(s) on {} entities", result.service_count(), result.entities.len());
    OK
}

fn catalog_type(base_type: &str, data_shape: Option<&str>) -> String {
    match data_shape {
        Some(shape) => format!("{base_type}<{shape}>"),
        None => base_type.to_string(),
    }
}

/// `twaco ext list | show | import | remove`: the server's extension packages.
fn ext_cmd(solution: &Solution, args: &Args) -> u8 {
    use twaco::core::extensions;
    let profile_name = args.profile.as_deref().unwrap_or("default");
    let client = match profile::load(&solution.root, profile_name) {
        Ok(profile) => server::Client::new(profile),
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let names: Vec<&str> = args.names.iter().map(String::as_str).collect();
    let result: Result<(), String> = (|| match names.as_slice() {
        ["list"] => {
            let packages = extensions::list(&client).map_err(|e| e.to_string())?;
            for p in &packages {
                if args.has("--json") {
                    println!(
                        "{}",
                        serde_json::json!({ "name": p.name, "version": p.version, "vendor": p.vendor, "minimumThingWorxVersion": p.minimum_thingworx, "group": p.group, "artifact": p.artifact })
                    );
                } else {
                    println!("{:<48} {:<20} {}", p.name, p.version, p.vendor);
                }
            }
            eprintln!("{} package(s)", packages.len());
            Ok(())
        }
        ["show", name] => {
            let shown = extensions::show(&client, name).map_err(|e| e.to_string())?;
            let p = &shown.package;
            println!("{} {} by {} (needs ThingWorx {})", p.name, p.version, if p.vendor.is_empty() { "?" } else { &p.vendor }, p.minimum_thingworx);
            if !p.description.is_empty() {
                println!("  {}", p.description);
            }
            println!("{} extension(s):", shown.extensions.len());
            for row in &shown.extensions {
                let name = row.get("name").and_then(serde_json::Value::as_str).unwrap_or("?");
                let kind = row.get("extensionType").or_else(|| row.get("type")).and_then(serde_json::Value::as_str).unwrap_or("");
                println!("  {name} {kind}");
            }
            println!("{} in use", shown.in_use.len());
            Ok(())
        }
        ["import", file] => {
            let zip = std::fs::read(file).map_err(|e| format!("{file}: {e}"))?;
            let file_name = std::path::Path::new(file).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "package.zip".into());
            let imported = extensions::import(&client, &file_name, &zip, args.has("--apply")).map_err(|e| e.to_string())?;
            if imported.applied {
                println!("done: {}; the package list now shows it", imported.plan);
            } else {
                println!("would {}; the server validated it and installed nothing (pass --apply)", imported.plan);
            }
            Ok(())
        }
        ["remove", name] => {
            let plan = extensions::remove(&client, name, args.has("--apply")).map_err(|e| e.to_string())?;
            if args.has("--apply") {
                println!("done: {plan}; it is gone from the package list");
            } else {
                println!("would {plan}; nothing sent (pass --apply)");
            }
            Ok(())
        }
        _ => Err("ext takes: list | show <package> | import <zip> | remove <package>".to_string()),
    })();
    match result {
        Ok(()) => OK,
        Err(why) => {
            eprintln!("twaco: ext: {why}");
            FAILED
        }
    }
}

/// `twaco repo list | ls | get | status`: the server's file repositories, read-only.
fn repo_cmd(solution: &Solution, args: &Args) -> u8 {
    use twaco::core::repo;
    let profile_name = args.profile.as_deref().unwrap_or("default");
    let client = match profile::load(&solution.root, profile_name) {
        Ok(profile) => server::Client::new(profile),
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let names: Vec<&str> = args.names.iter().map(String::as_str).collect();
    let result: Result<(), String> = (|| match names.as_slice() {
        ["list"] => {
            for name in repo::Remote::repositories(&client).map_err(|e| e.to_string())? {
                println!("{name}");
            }
            Ok(())
        }
        ["ls", repository, rest @ ..] if rest.len() <= 1 => {
            let folder = rest.first().copied().unwrap_or("/");
            let listing = repo::list(&client, repository, folder, args.has("--recursive")).map_err(|e| e.to_string())?;
            if args.has("--json") {
                for folder in &listing.folders {
                    println!("{}", serde_json::json!({ "path": folder, "type": "folder" }));
                }
                for file in &listing.files {
                    println!("{}", serde_json::json!({ "path": file.path, "type": "file", "size": file.size, "modified": twaco::core::logs::iso(file.modified) }));
                }
            } else {
                for folder in &listing.folders {
                    println!("{folder}/");
                }
                for file in &listing.files {
                    println!("{:>12}  {}  {}", file.size, twaco::core::logs::local(file.modified), file.path);
                }
            }
            eprintln!("{} folder(s), {} file(s)", listing.folders.len(), listing.files.len());
            Ok(())
        }
        ["get", repository, path] => {
            let bytes = repo::get(&client, repository, path).map_err(|e| e.to_string())?;
            match &args.out {
                None => {
                    use std::io::Write;
                    std::io::stdout().write_all(&bytes).map_err(|e| e.to_string())
                }
                Some(out) => {
                    if out.exists() && !args.has("--force") {
                        return Err(format!("{} exists; pass --force to replace it", out.display()));
                    }
                    workspace::write_entity(out, &bytes).map_err(|e| e.to_string())?;
                    eprintln!("{} bytes to {}", bytes.len(), out.display());
                    Ok(())
                }
            }
        }
        ["status", repository] => {
            let root = repo::local_root(&solution.root, solution.repositories.root.as_deref(), repository);
            let compared = repo::status(&client, repository, &root).map_err(|e| e.to_string())?;
            let mut counts = std::collections::BTreeMap::new();
            for item in &compared {
                *counts.entry(item.state.label()).or_insert(0usize) += 1;
                if args.has("--json") {
                    println!(
                        "{}",
                        serde_json::json!({ "path": item.path, "state": item.state.label(), "local_size": item.local_size, "remote_size": item.remote_size })
                    );
                } else if item.state != repo::State::Same {
                    println!("{:<11} {}", item.state.label(), item.path);
                }
            }
            let summary: Vec<String> = counts.iter().map(|(state, n)| format!("{n} {state}")).collect();
            eprintln!(
                "{} against {}: {}",
                repository,
                root.display(),
                if summary.is_empty() { "both empty".to_string() } else { summary.join(", ") }
            );
            Ok(())
        }
        ["put", repository, file, path] => {
            let bytes = std::fs::read(file).map_err(|e| format!("{file}: {e}"))?;
            let path = repo::remote_path(path).map_err(|e| e.to_string())?;
            repo_change(&client, repository, &repo::Change::Put { path, bytes, overwrite: args.has("--overwrite") }, args)
        }
        ["mkdir", repository, path] => {
            let path = repo::remote_path(path).map_err(|e| e.to_string())?;
            repo_change(&client, repository, &repo::Change::Mkdir { path }, args)
        }
        ["rm", repository, path] => {
            let path = repo::remote_path(path).map_err(|e| e.to_string())?;
            if path == "/" {
                return Err("the repository root cannot be deleted".to_string());
            }
            repo_change(&client, repository, &repo::Change::Remove { path, recursive: args.has("--recursive") }, args)
        }
        [direction @ ("push" | "pull"), repository] => {
            let root = repo::local_root(&solution.root, solution.repositories.root.as_deref(), repository);
            let way = if *direction == "push" { repo::Direction::Push } else { repo::Direction::Pull };
            let apply = args.has("--apply");
            let synced = repo::sync(&client, repository, &root, way, args.has("--overwrite"), apply).map_err(|e| e.to_string())?;
            let verb = match (way, apply) {
                (repo::Direction::Push, true) => "uploaded",
                (repo::Direction::Push, false) => "would upload",
                (repo::Direction::Pull, true) => "downloaded",
                (repo::Direction::Pull, false) => "would download",
            };
            for path in &synced.copied {
                println!("{verb} {path}");
            }
            for path in &synced.left {
                println!("left alone (only {}) {path}", if way == repo::Direction::Push { "on the server" } else { "here" });
            }
            println!(
                "{verb} {} file(s); {} the same; {} left alone{}",
                synced.copied.len(),
                synced.same,
                synced.left.len(),
                if apply || synced.copied.is_empty() { "" } else { "; nothing sent (pass --apply)" }
            );
            Ok(())
        }
        ["mv", repository, from, to] => {
            let from = repo::remote_path(from).map_err(|e| e.to_string())?;
            let to = repo::remote_path(to).map_err(|e| e.to_string())?;
            repo_change(&client, repository, &repo::Change::Move { from, to, overwrite: args.has("--overwrite") }, args)
        }
        _ => Err("repo takes: list | ls <repo> [<path>] | get <repo> <path> | status <repo> | put <repo> <file> <path> | mkdir <repo> <path> | rm <repo> <path> | mv <repo> <from> <to>".to_string()),
    })();
    match result {
        Ok(()) => OK,
        Err(why) => {
            eprintln!("twaco: repo: {why}");
            FAILED
        }
    }
}

/// One repository change: a plan unless --apply, applied and read back with it.
fn repo_change(
    client: &server::Client,
    repository: &str,
    change: &twaco::core::repo::Change,
    args: &Args,
) -> Result<(), String> {
    let planned = twaco::core::repo::change(client, repository, change, args.has("--apply")).map_err(|e| e.to_string())?;
    if planned.nothing {
        println!("{}: nothing to do", planned.plan);
    } else if planned.applied {
        println!("done: {} in {repository}, read back", planned.plan);
    } else {
        println!("would {} in {repository}; nothing sent (pass --apply)", planned.plan);
    }
    Ok(())
}

fn is_info_table(value: &serde_json::Value) -> bool {
    value.get("dataShape").is_some() && value.get("rows").is_some_and(serde_json::Value::is_array)
}

fn print_info_table_summary(value: &serde_json::Value) {
    let rows = value["rows"].as_array().expect("is_info_table checked rows");
    let fields: Vec<&str> = value["dataShape"]
        .get("fieldDefinitions")
        .and_then(serde_json::Value::as_object)
        .map(|fields| fields.keys().map(String::as_str).collect())
        .or_else(|| {
            rows.first()
                .and_then(serde_json::Value::as_object)
                .map(|row| row.keys().map(String::as_str).collect())
        })
        .unwrap_or_default();
    println!("{} row(s)", rows.len());
    println!("fields: {}", fields.join(", "));
    if let Some(first) = rows.first() {
        println!("first row:");
        println!("{}", serde_json::to_string_pretty(first).expect("JSON value serialises"));
    }
}

/// Load the solution once, then hand it to a command.
/// Whether a command changes the workspace: its entity files, sidecars, bundle or baseline.
///
/// Those take the workspace lock. Everything else runs alongside them, so `entity status` can
/// watch a deploy in progress. `config-table` writes only to the server and to a backup file of
/// the user's choosing, and `entity get --out` writes a file the user named, so neither does.
fn writes_workspace(route: &str, args: &Args) -> bool {
    match route {
        "extract" | "types" => true,
        "sync" | "fmt" | "bundle" => !args.has("--check"),
        "deploy" | "entity push" | "adopt" | "rename entity" | "rename prefix" | "rename field" | "rename service" | "rename param" | "rename table" | "rename property" | "move service" | "move property" | "copy service" | "copy property" | "retemplate" | "new building-block" => args.has("--apply"),
        "entity status" => args.has("--record"),
        // Even with --apply, db run writes only a throwaway server Thing and needs no workspace lock.
        "db run" => false,
        // A pull writes the repository's tree into the solution; everything else in repo
        // writes only to the server, or to a file the user named.
        "repo" => args.names.first().is_some_and(|n| n == "pull") && args.has("--apply"),
        _ => false,
    }
}

/// Take the lock, sweeping stale temporaries from everywhere twaco writes through one.
fn take_lock(solution: &Solution, route: &str) -> Result<lock::WorkspaceLock, u8> {
    match lock::acquire_for(solution, route) {
        Ok(lock) => {
            for path in &lock.recovered {
                eprintln!("twaco: removed {}, left by an interrupted write", path.display());
            }
            Ok(lock)
        }
        Err(error) => {
            eprintln!("twaco: {error}");
            Err(FAILED)
        }
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

fn types_cmd(solution: &Solution, args: &Args) -> u8 {
    if args.has("--check") && args.has("--platform") {
        eprintln!("twaco: types: --check and --platform cannot be used together");
        return FAILED;
    }
    if args.has("--json") && !args.has("--check") {
        eprintln!("twaco: types: --json requires --check");
        return FAILED;
    }
    if args.has("--platform") {
        let profile_name = args.profile.as_deref().unwrap_or("default");
        let profile = match profile::load(&solution.root, profile_name) {
            Ok(profile) => profile,
            Err(error) => {
                eprintln!("twaco: types: {error}");
                return FAILED;
            }
        };
        return match types::fetch_platform(&server::Client::new(profile), solution) {
            Ok(outcome) => {
                for skipped in outcome.skipped.iter().chain(&outcome.types.skipped) {
                    eprintln!("twaco: skipped {skipped}");
                }
                println!(
                    "fetched {} templates, {} shapes and {} resources from the server into .twaco/platform.json",
                    outcome.templates, outcome.shapes, outcome.resources
                );
                OK
            }
            Err(error) => {
                eprintln!("twaco: types: {error}");
                FAILED
            }
        };
    }
    if args.has("--check") {
        return match types::check(solution) {
            Ok(outcome) => {
                for skipped in &outcome.declarations.skipped {
                    eprintln!("twaco: skipped {skipped}");
                }
                for finding in &outcome.findings {
                    if args.has("--json") {
                        println!("{}", types::finding_json(finding));
                    } else {
                        println!("{}:{}:{}: TS{} {}", finding.file, finding.line, finding.column, finding.code, finding.message);
                    }
                }
                if args.has("--json") {
                    eprintln!("{}", types::check_summary(&outcome));
                } else {
                    println!("{}", types::check_summary(&outcome));
                }
                if outcome.findings.is_empty() { OK } else { DRIFT }
            }
            Err(error) => {
                eprintln!("twaco: types: {error}");
                FAILED
            }
        };
    }
    match types::write(solution) {
        Ok(outcome) => {
            for skipped in &outcome.skipped {
                eprintln!("twaco: skipped {skipped}");
            }
            println!(
                "typed {} entities, {} DataShapes and {} services ({} files written)",
                outcome.entities, outcome.data_shapes, outcome.services, outcome.files_written
            );
            if !outcome.gitignore_covers_types {
                eprintln!(
                    "twaco: note: add `.twaco/types/`, `**/services/*/jsconfig.json`, and \
                     `**/services/*/twaco-globals.d.ts` to the solution root's .gitignore"
                );
            }
            OK
        }
        Err(error) => {
            eprintln!("twaco: types: {error}");
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
    let label = if solution.solution.name.is_empty() { "(unnamed)" } else { &solution.solution.name };
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
            println!("  {} says {} but is filed under {}", entity.info.name, entity.info.project, entity.found_under);
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
        chosen.push(workspace::resolve(&pool, name).map_err(|e| e.to_string())?.clone());
    }
    Ok((chosen, found.unreadable))
}

/// Fetch one raw export without ever replacing a file that belongs to the solution.
fn entity_get(solution: &Solution, args: &Args) -> u8 {
    if args.names.len() != 1 {
        eprintln!("twaco: entity get needs exactly one entity name");
        return FAILED;
    }
    let (chosen, unreadable) = match targets(solution, args) {
        Ok(result) => result,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    if !unreadable.is_empty() {
        for problem in unreadable {
            eprintln!("twaco: {problem}");
        }
        return FAILED;
    }
    let entity = &chosen[0];
    if let Some(out) = &args.out {
        let protected = workspace::entities(solution);
        if protected.iter().any(|candidate| same_path(out, &candidate.path)) {
            eprintln!(
                "twaco: --out {} is a project entity file; entity get never overwrites project source",
                out.display()
            );
            return FAILED;
        }
    }

    let profile_name = args.profile.as_deref().unwrap_or("default");
    let profile = match profile::load(&solution.root, profile_name) {
        Ok(profile) => profile,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let live = match server::Client::new(profile).fetch_entity(&entity.info.collection, &entity.info.name) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };

    if let Some(out) = &args.out {
        if let Err(error) = workspace::write_entity(out, &live) {
            eprintln!("twaco: {error}");
            return FAILED;
        }
        println!("wrote {} raw bytes to {}", live.len(), out.display());
    } else if let Err(error) = std::io::stdout().write_all(&live) {
        eprintln!("twaco: stdout: {error}");
        return FAILED;
    }
    OK
}

/// Compare working, server and tracked ancestor, optionally recording matching hashes once.
fn entity_status(solution: &Solution, args: &Args) -> u8 {
    if args.names.len() > 1 {
        eprintln!("twaco: entity status accepts one entity name, or --all");
        return FAILED;
    }
    let (chosen, unreadable) = match targets(solution, args) {
        Ok(result) => result,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    if !unreadable.is_empty() {
        for problem in unreadable {
            eprintln!("twaco: {problem}");
        }
        return FAILED;
    }
    let profile_name = args.profile.as_deref().unwrap_or("default");
    let profile = match profile::load(&solution.root, profile_name) {
        Ok(profile) => profile,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let client = server::Client::new(profile);
    let mut baseline = match baseline::Baseline::load(&solution.root) {
        Ok(baseline) => baseline,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };

    let (statuses, failures) = status::compute(&client, &baseline, &chosen);
    if !failures.is_empty() {
        for failure in &failures {
            eprintln!("twaco: {failure}");
        }
        eprintln!("twaco: {} entity status request(s) failed", failures.len());
        return FAILED;
    }

    println!("{} entity status(es)", statuses.len());
    for (verdict, count) in status::counts(&statuses) {
        println!("  {:<19} {count}", verdict.label());
    }
    if args.has("--detail") {
        println!();
        for status in &statuses {
            println!("{}/{}  {}", status.collection, status.name, status.verdict.label());
            println!("  working  {}", status.working);
            println!("  server   {}", status.server.as_deref().unwrap_or("-"));
            println!("  baseline local  {}", status.local_baseline.as_deref().unwrap_or("-"));
            println!("  baseline server {}", status.server_baseline.as_deref().unwrap_or("-"));
        }
    }
    if args.has("--record") {
        status::record_matching(&mut baseline, &statuses);
        if let Err(error) = baseline.write(&solution.root) {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    }
    if statuses.iter().all(|status| !status.verdict.is_drift()) {
        OK
    } else {
        DRIFT
    }
}

/// Push one entity. A dry run unless `--apply`: the blast radius is a server.
fn entity_push(solution: &Solution, args: &Args) -> u8 {
    if args.names.len() != 1 || args.has("--all") {
        eprintln!("twaco: entity push takes exactly one entity name");
        return FAILED;
    }
    let (chosen, unreadable) = match targets(solution, args) {
        Ok(result) => result,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    if !unreadable.is_empty() {
        for problem in unreadable {
            eprintln!("twaco: {problem}");
        }
        return FAILED;
    }
    let entity = &chosen[0];
    let bytes = match std::fs::read(&entity.path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("twaco: {}: {error}", entity.path.display());
            return FAILED;
        }
    };
    let profile_name = args.profile.as_deref().unwrap_or("default");
    let profile = match profile::load(&solution.root, profile_name) {
        Ok(profile) => profile,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let file_name = entity
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| format!("{}.xml", entity.info.name));
    let target = push::Target {
        collection: &entity.info.collection,
        name: &entity.info.name,
        file_name: &file_name,
        bytes: &bytes,
    };
    let apply = args.has("--apply");
    let force = args.has("--force");
    let label = format!("{}/{}", entity.info.collection, entity.info.name);
    let client = server::Client::new(profile);
    if apply && force && !args.has("--no-backup") {
        match backup::before_forced_push(&client, solution, &target, &backup::new_stamp()) {
            Ok(Some(dir)) => println!("{label}: the server's copy was saved to {dir} before it is overwritten"),
            Ok(None) => {}
            Err(error) => {
                eprintln!("twaco: {label}: {error} (--no-backup pushes without one)");
                return FAILED;
            }
        }
    }
    match push::push(&client, &solution.root, &target, apply, force) {
        Ok(push::Outcome::WouldDo(decision)) => {
            match decision {
                push::Decision::AlreadyThere => {
                    println!("{label}: nothing to push")
                }
                push::Decision::Create => println!("{label}: would create it on the server"),
                push::Decision::Update => {
                    println!("{label}: would update it; the server is unchanged since the last sync")
                }
                push::Decision::Refuse(refusal) => {
                    println!("{label}: would refuse: {refusal}");
                    if !force {
                        return DRIFT;
                    }
                    println!("  --force would push anyway");
                }
            }
            println!("dry run: nothing was sent; pass --apply to push");
            OK
        }
        Ok(push::Outcome::AlreadyThere) => {
            println!("{label}: nothing to push; baseline is current");
            OK
        }
        Ok(push::Outcome::Pushed { created }) => {
            let verb = if created { "created" } else { "updated" };
            println!("{label}: {verb}, read back and matching; baseline recorded");
            OK
        }
        Ok(push::Outcome::Refused(refusal)) => {
            eprintln!("twaco: {label}: refused: {refusal}");
            eprintln!("twaco: nothing was sent; --force pushes anyway");
            DRIFT
        }
        Err(error) => {
            eprintln!("twaco: {label}: {error}");
            FAILED
        }
    }
}

/// Delete server entities in dependency-safe order. Planning is read-only and always succeeds
/// even when it reports guarded refusals; an apply reports any refusal or failed confirmation as
/// exit 2 after attempting the rest.
fn entity_delete_cmd(solution: &Solution, args: &Args) -> u8 {
    let apply = args.has("--apply");
    let (acknowledged, force_used) = entity_delete::acknowledged(
        args.has("--force"),
        args.has("--allow-repository-defined"),
        args.has("--allow-outside-dependents"),
        args.has("--allow-file-repository-data-loss"),
    );
    if force_used {
        eprintln!("twaco: {}", entity_delete_force_deprecation());
    }
    let prepared = match entity_delete::prepare(solution, &args.names, args.has("--renamed")) {
        Ok(prepared) => prepared,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    // Server-only deletion needs no workspace lock. The ledger is the sole local write, and an
    // empty/already-complete ledger does not manufacture a writer where there is none.
    let _lock = if prepared.ledger_will_be_written(apply) {
        match take_lock(solution, "entity delete") {
            Ok(lock) => Some(lock),
            Err(code) => return code,
        }
    } else {
        None
    };
    let profile_name = args.profile.as_deref().unwrap_or("default");
    let profile = match profile::load(&solution.root, profile_name) {
        Ok(profile) => profile,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let date = jiff::Zoned::now().strftime("%Y-%m-%d").to_string();
    let prepared = if args.has("--no-backup") { prepared } else { prepared.with_backup(&backup::new_stamp()) };
    let report = match entity_delete::run(
        &server::Client::new(profile),
        solution,
        prepared,
        apply,
        acknowledged,
        &date,
    ) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    if args.has("--json") {
        let key = if apply { "applied" } else { "plan" };
        let value = serde_json::json!({ (key): true, "entities": report.entities, "backup": report.backup });
        println!("{}", serde_json::to_string_pretty(&value).expect("delete report serialises"));
    } else {
        println!("{}:", if apply { "applied" } else { "plan" });
        for (at, entity) in report.entities.iter().enumerate() {
            println!(
                "  {}. {}/{}  {:?}  {}",
                at + 1,
                entity.collection,
                entity.name,
                entity.status,
                entity.method
            );
            for (code, refusal) in entity.refusal_pairs() {
                println!("     refused [{}]: {refusal}", code.as_str());
            }
            for dependent in &entity.dependents {
                println!("     dependent: {}/{}", dependent.collection, dependent.name);
            }
            for warning in &entity.warnings {
                println!("     warning: {warning}");
            }
            if let Some(error) = &entity.error {
                println!("     failed: {error}");
            }
        }
        println!("limit: {}", report.dependency_limit);
        if let Some(dir) = &report.backup {
            println!("backup: the server's copies were saved to {dir}; `twaco entity restore` puts them back");
        }
        if !apply {
            println!("dry run: nothing was deleted; pass --apply to delete");
        } else if report.ledger_changed {
            println!("rename ledger marked with {date}");
        }
    }
    if apply && report.failed() { FAILED } else { OK }
}

fn entity_delete_force_deprecation() -> &'static str {
    entity_delete::FORCE_DEPRECATION
}

/// Create a building block as files and register its project. Plans unless --apply.
fn new_building_block_cmd(solution: &Solution, args: &Args) -> u8 {
    let [name] = args.names.as_slice() else {
        eprintln!("twaco: new building-block needs one <name>, such as Acme.Orders");
        return FAILED;
    };
    let kind = match args.values.get("--type") {
        None => newblock::BlockType::Standard,
        Some(word) => match newblock::BlockType::from_word(word) {
            Some(kind) => kind,
            None => {
                eprintln!("twaco: --type is standard, abstract or implementation, not {word:?} (ui and test blocks are not created here yet)");
                return FAILED;
            }
        },
    };
    let request = newblock::Request {
        name: name.clone(),
        kind,
        display_name: args.values.get("--display-name").cloned(),
        description: args.values.get("--description").cloned().unwrap_or_default(),
        parent: args.values.get("--parent").cloned(),
        model_logic: args.has("--model-logic"),
        management_shape: !args.has("--no-management-shape"),
        root: args.values.get("--root").cloned(),
        base_extension: args.values.get("--base-extension").cloned(),
    };
    let plan = match newblock::plan(solution, &request) {
        Ok(plan) => plan,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let apply = args.has("--apply");
    if apply {
        if let Err(error) = newblock::apply(solution, &plan) {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    }
    let files: Vec<String> = plan.files.iter().map(|file| file.path.strip_prefix(&solution.root).unwrap_or(&file.path).display().to_string().replace('\\', "/")).collect();
    if args.has("--json") {
        let value = serde_json::json!({
            (if apply { "applied" } else { "plan" }): true,
            "name": request.name, "type": request.kind.word(), "root": plan.root,
            "files": files, "twaco_toml": plan.config_addition, "notes": plan.notes,
        });
        println!("{}", serde_json::to_string_pretty(&value).expect("new block plan serialises"));
        return OK;
    }
    println!("new building-block {} ({}): {}", request.name, request.kind.word(), if apply { "created" } else { "a plan, nothing was written (pass --apply)" });
    for file in &files {
        println!("  {file}");
    }
    println!("  twaco.toml gets:{}", plan.config_addition.trim_end().replace('\n', "\n    "));
    for note in &plan.notes {
        println!("  note: {note}");
    }
    if apply {
        println!("Next: twaco check; commit; twaco deploy --apply to create it on the server.");
    }
    OK
}

/// Change a template or implemented shapes. Plans unless --apply; a loss that holds data or is
/// still referenced is refused unless --accept-loss.
fn retemplate_cmd(solution: &Solution, args: &Args) -> u8 {
    let [entity] = args.names.as_slice() else {
        eprintln!("twaco: retemplate needs one <entity>");
        return FAILED;
    };
    let list = |flag: &str| -> Vec<String> {
        args.values.get(flag).map(|text| text.split(',').map(str::trim).filter(|item| !item.is_empty()).map(str::to_string).collect()).unwrap_or_default()
    };
    let request = retemplate::Request {
        entity: entity.clone(),
        template: args.values.get("--to").cloned(),
        add_shapes: list("--add-shapes"),
        remove_shapes: list("--remove-shapes"),
        accept_loss: args.has("--accept-loss"),
    };
    let plan = match retemplate::plan(solution, &request) {
        Ok(plan) => plan,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let apply = args.has("--apply");
    if apply {
        if let Err(error) = retemplate::apply(&plan) {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    }
    if args.has("--json") {
        let value = serde_json::json!({
            (if apply { "applied" } else { "plan" }): true,
            "entity": plan.request.entity, "collection": plan.collection, "file": plan.file_relative(solution),
            "affected": plan.affected, "gained": plan.gained, "lost": plan.lost,
            "needs_accept_loss": plan.blocked, "notes": plan.notes,
        });
        println!("{}", serde_json::to_string_pretty(&value).expect("retemplate plan serialises"));
        return OK;
    }
    println!("retemplate {}: {}", plan.request.entity, if apply { "applied" } else { "a plan, nothing was written (pass --apply)" });
    println!("  {}", plan.file_relative(solution));
    let detail = args.has("--detail");
    let shown = |items: &[retemplate::Change]| -> String {
        if items.is_empty() {
            return "nothing".to_string();
        }
        let count = |kind: &str| items.iter().filter(|change| change.kind == kind).count();
        let summary = format!("{} (services {}, properties {}, configuration tables {})", items.len(), count("service"), count("property"), count("configuration table"));
        let limit = if detail { items.len() } else { items.len().min(6) };
        let names = items.iter().take(limit).map(|change| change.name.as_str()).collect::<Vec<_>>().join(", ");
        format!("{summary}: {names}{}", if limit < items.len() { ", ..." } else { "" })
    };
    println!("  affects        {} entit{}", plan.affected.len(), if plan.affected.len() == 1 { "y" } else { "ies" });
    println!("  gains          {}", shown(&plan.gained));
    println!("  loses          {}", shown(&plan.lost));
    for change in &plan.lost {
        if change.orphaned > 0 {
            println!("  holds          {} {} is held by {} entit{}", change.kind, change.name, change.orphaned, if change.orphaned == 1 { "y" } else { "ies" });
        }
        let limit = if detail { change.references.len() } else { change.references.len().min(3) };
        for line in change.references.iter().take(limit) {
            println!("  references     {} {}: {line}", change.kind, change.name);
        }
    }
    for note in &plan.notes {
        println!("  note: {note}");
    }
    if !plan.blocked.is_empty() && !apply {
        let shown_reasons = if detail { plan.blocked.len() } else { plan.blocked.len().min(3) };
        println!("needs --accept-loss: {} reason(s): {}{}", plan.blocked.len(), plan.blocked[..shown_reasons].join("; "), if shown_reasons < plan.blocked.len() { "; ..." } else { "" });
    }
    if apply {
        println!("Next: twaco check; commit; twaco deploy --apply.");
    }
    OK
}

/// Move or copy a service or property between entities. Plans unless --apply; an apply checks the
/// sidecars still match the XML and exits 2 if they do not.
fn relocate_cmd(solution: &Solution, route: &str, args: &Args) -> u8 {
    let (verb, word) = route.split_once(' ').expect("the route has a verb and a member");
    let member = relocate::Member::from_word(word).expect("the route names a member");
    let [from, to, name] = args.names.as_slice() else {
        eprintln!("twaco: {route} needs <from> <to> <name>");
        return FAILED;
    };
    let request = relocate::Request {
        member,
        copy: verb == "copy",
        from: from.clone(),
        to: to.clone(),
        name: name.clone(),
        new_name: args.values.get("--as").cloned(),
        leave_delegate: args.has("--leave-delegate"),
    };
    let apply = args.has("--apply");
    let plan = match relocate::plan(solution, &request) {
        Ok(plan) => plan,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let mut problems = Vec::new();
    if apply {
        if let Err(error) = relocate::apply(&plan) {
            eprintln!("twaco: {error}");
            return FAILED;
        }
        problems = relocate::verify(solution, &plan);
    }
    if args.has("--json") {
        let value = serde_json::json!({
            (if apply { "applied" } else { "plan" }): true,
            "action": verb, "member": word, "from": plan.request.from, "to": plan.request.to,
            "name": plan.request.name, "as": plan.final_name,
            "files": plan.files(solution), "callers": plan.callers, "notes": plan.notes,
            "out_of_step": problems,
        });
        println!("{}", serde_json::to_string_pretty(&value).expect("relocation serialises"));
    } else {
        println!("{verb} {word} {}.{} -> {}.{}: {}", plan.request.from, plan.request.name, plan.request.to, plan.final_name, if apply { "applied" } else { "a plan, nothing was written (pass --apply)" });
        for file in plan.files(solution) {
            println!("  {file}");
        }
        for note in &plan.notes {
            println!("  note: {note}");
        }
        let shown = if args.has("--detail") { plan.callers.first.len() } else { plan.callers.first.len().min(3) };
        for line in plan.callers.first.iter().take(shown) {
            println!("  caller: {line}");
        }
        for entity in &problems {
            println!("  out of step: {entity} needs attention");
        }
        if apply {
            println!("Next: twaco check; commit; twaco deploy --apply. Property values stored on Things are not moved.");
        }
    }
    if problems.is_empty() { OK } else { FAILED }
}

/// List backup sets, or plan (and with --apply perform) importing one back.
fn entity_restore_cmd(solution: &Solution, args: &Args) -> u8 {
    let json = args.has("--json");
    let Some((id, only)) = args.names.split_first() else {
        let sets = backup::list(solution);
        if json {
            let value = serde_json::json!({ "sets": sets.iter().map(|set| serde_json::json!({
                "id": set.id, "created": set.manifest.created, "reason": set.manifest.reason, "entities": set.manifest.entities.len(),
            })).collect::<Vec<_>>() });
            println!("{}", serde_json::to_string_pretty(&value).expect("sets serialise"));
        } else if sets.is_empty() {
            println!("no backup sets under {}", backup::DIR);
        } else {
            for set in &sets {
                println!("{}  {}  {} entit{}", set.id, set.manifest.reason, set.manifest.entities.len(), if set.manifest.entities.len() == 1 { "y" } else { "ies" });
            }
        }
        return OK;
    };
    let set = match backup::find(solution, id) {
        Ok(set) => set,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let profile_name = args.profile.as_deref().unwrap_or("default");
    let profile = match profile::load(&solution.root, profile_name) {
        Ok(profile) => profile,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let apply = args.has("--apply");
    let report = match backup::restore(&server::Client::new(profile), &set, only, apply) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    if json {
        let value = serde_json::json!({ (if apply { "applied" } else { "plan" }): true, "set": set.id, "entities": report });
        println!("{}", serde_json::to_string_pretty(&value).expect("restore report serialises"));
    } else {
        println!("{} of {}:", if apply { "applied" } else { "plan" }, set.id);
        for entry in &report {
            println!("  {}/{}  {:?}", entry.collection, entry.name, entry.status);
            if let Some(error) = &entry.error {
                println!("     failed: {error}");
            }
        }
        if !apply {
            println!("dry run: nothing was imported; pass --apply to restore");
        }
    }
    if report.iter().any(|entry| entry.status == backup::Status::Failed) { FAILED } else { OK }
}

/// Copy permissions from old entities to the ones that replaced them. A plan only reads; an apply
/// writes what differs and reports any failure as exit 2 after attempting the rest. Only an apply
/// that reads the ledger's pending entries writes the workspace, and so takes its lock.
fn entity_carry_cmd(solution: &Solution, args: &Args) -> u8 {
    let apply = args.has("--apply");
    let renamed = args.has("--renamed");
    let pairs = match entity_carry::pairs_from_names(&args.names) {
        Ok(pairs) => pairs,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let _lock = if apply && renamed {
        match take_lock(solution, "entity carry") {
            Ok(lock) => Some(lock),
            Err(code) => return code,
        }
    } else {
        None
    };
    let profile_name = args.profile.as_deref().unwrap_or("default");
    let profile = match profile::load(&solution.root, profile_name) {
        Ok(profile) => profile,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let date = jiff::Zoned::now().strftime("%Y-%m-%d").to_string();
    let request = entity_carry::Request { pairs, renamed, apply, detail: args.has("--detail") };
    let report = match entity_carry::run(&server::Client::new(profile), solution, &request, &date) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    if args.has("--json") {
        let key = if apply { "applied" } else { "plan" };
        let value = serde_json::json!({ (key): true, "entities": report.entities });
        println!("{}", serde_json::to_string_pretty(&value).expect("carry report serialises"));
    } else {
        println!("{}:", if apply { "applied" } else { "plan" });
        for entity in &report.entities {
            let kinds = if entity.kinds.is_empty() { String::new() } else { format!("  [{}]", entity.kinds.join(", ")) };
            println!("  {}/{} -> {}  {:?}{kinds}", entity.collection, entity.old, entity.new, entity.status);
            if let Some(count) = entity.differences {
                println!("     the platform reports {count} difference(s) between them");
            }
            if let Some(error) = &entity.error {
                println!("     failed: {error}");
            }
        }
        if !apply {
            println!("dry run: nothing was written; pass --apply to carry");
        } else if report.ledger_changed {
            println!("rename ledger marked with {date}");
        }
    }
    if report.entities.iter().any(|entity| entity.status == entity_carry::Status::Failed) && apply { FAILED } else { OK }
}

/// Copy a DataTable's rows into the one that replaced it. A plan reads; an apply writes, then
/// reads the target back and compares.
fn datatable_copy_cmd(solution: &Solution, args: &Args) -> u8 {
    let [old, new] = args.names.as_slice() else {
        eprintln!("twaco: datatable copy needs <old> <new> DataTable names");
        return FAILED;
    };
    let map = match args.values.get("--map").map(|text| datatable_copy::parse_map(text)).transpose() {
        Ok(map) => map.unwrap_or_default(),
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let max_rows = match args.values.get("--max-rows").map(|value| value.parse::<u64>()) {
        None => 100_000,
        Some(Ok(value)) if value > 0 => value,
        Some(_) => {
            eprintln!("twaco: --max-rows needs a positive whole number");
            return FAILED;
        }
    };
    let profile_name = args.profile.as_deref().unwrap_or("default");
    let profile = match profile::load(&solution.root, profile_name) {
        Ok(profile) => profile,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let apply = args.has("--apply");
    let request = datatable_copy::Request {
        old: old.clone(),
        new: new.clone(),
        map,
        drop_unmapped: args.has("--drop-unmapped"),
        append: args.has("--append"),
        max_rows,
        apply,
    };
    let report = match datatable_copy::run(&server::Client::new(profile), solution, &request) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    if args.has("--json") {
        let value = serde_json::to_value(&report).expect("copy report serialises");
        println!("{}", serde_json::to_string_pretty(&value).expect("copy report serialises"));
        return OK;
    }
    println!("{}: {} -> {}", if apply { "applied" } else { "plan" }, report.old, report.new);
    println!("  rows: {} to copy, {} already in the target", report.source_rows, report.target_rows_before);
    for field in &report.fields {
        println!("  field {} -> {}  ({})", field.from, field.to, field.by);
    }
    for field in &report.dropped {
        println!("  field {field} left behind");
    }
    for field in &report.unfilled {
        println!("  field {field} of the target is not filled");
    }
    if apply {
        println!("  wrote {} row(s); read back equal: {}", report.written, report.verified);
        println!("  not carried: each row's source, tags and timestamp (the write stamps the caller and the time)");
    } else {
        println!("dry run: nothing was written; pass --apply to copy");
    }
    OK
}

/// Delete the temporary Things an interrupted `db run` or `db query` left on the server. Plans by
/// default; touches only names twaco generates, on `Database` Things.
fn db_clean_cmd(solution: &Solution, args: &Args) -> u8 {
    let apply = args.has("--apply");
    let profile_name = args.profile.as_deref().unwrap_or("default");
    let profile = match profile::load(&solution.root, profile_name) {
        Ok(profile) => profile,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let swept = match db::sweep(&server::Client::new(profile), apply) {
        Ok(swept) => swept,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    if args.has("--json") {
        let key = if apply { "applied" } else { "plan" };
        let value = serde_json::json!({ (key): true, "things": swept });
        println!("{}", serde_json::to_string_pretty(&value).expect("sweep report serialises"));
    } else {
        println!("{}:", if apply { "applied" } else { "plan" });
        for thing in &swept {
            let why = thing.why.as_ref().map(|why| format!("  ({why})")).unwrap_or_default();
            println!("  {}  {:?}{why}", thing.name, thing.status);
        }
        if swept.is_empty() {
            println!("  no temporary Things on the server");
        } else if !apply {
            println!("dry run: nothing was deleted; pass --apply to delete the stale ones");
        }
    }
    if swept.iter().any(|thing| thing.status == db::SweepStatus::Failed) { FAILED } else { OK }
}

fn db_cmd(solution: &Solution, route: &str, args: &Args) -> u8 {
    let mode = if route == "db run" { db::Mode::Run } else { db::Mode::Query };
    let inline = args.values.get("-q");
    let sql = match (mode, args.names.as_slice(), inline) {
        (db::Mode::Run, [file], None) | (db::Mode::Query, [file], None) => {
            match std::fs::read_to_string(file) {
                Ok(sql) => sql,
                Err(error) => {
                    eprintln!("twaco: cannot read SQL file {file}: {error}");
                    return FAILED;
                }
            }
        }
        (db::Mode::Query, [], Some(sql)) => sql.clone(),
        (db::Mode::Run, _, Some(_)) => {
            eprintln!("twaco: db run needs one <file.sql>; -q belongs to db query");
            return FAILED;
        }
        (db::Mode::Query, _, _) => {
            eprintln!("twaco: db query needs one <file.sql>, or -q <sql>, but not both");
            return FAILED;
        }
        _ => {
            eprintln!("twaco: db run needs one <file.sql>");
            return FAILED;
        }
    };
    let max_rows = match args.values.get("--max-rows") {
        Some(value) => match value.parse::<u64>() {
            Ok(value) if value > 0 => value,
            _ => {
                eprintln!("twaco: --max-rows needs a positive whole number");
                return FAILED;
            }
        },
        None => 500,
    };
    let profile_name = args.profile.as_deref().unwrap_or("default");
    let profile = match profile::load(&solution.root, profile_name) {
        Ok(profile) => profile,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let options = db::Options {
        mode,
        thing: args.values.get("--thing").cloned(),
        apply: mode == db::Mode::Query || args.has("--apply"),
        no_transaction: args.has("--no-transaction"),
        max_rows,
        timeout: args.timeout.unwrap_or(Duration::from_secs(120)),
    };
    let report = match db::execute(&server::Client::new(profile.clone()), solution, &profile, &sql, &options) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    if args.has("--json") {
        let mut value = serde_json::to_value(&report).expect("db report serialises");
        if mode == db::Mode::Query {
            summarise_db_json(&mut value, args.has("--detail"));
        }
        println!("{}", serde_json::to_string_pretty(&value).expect("db report serialises"));
        return OK;
    }
    if !report.applied {
        println!("target Thing: {}", report.thing);
        println!("JDBC URL: {}", report.jdbc_url);
        println!("user: {}", report.user);
        println!("SQL: {} bytes", report.bytes);
        println!("{}", report.sql);
        println!("dry run: nothing was sent; pass --apply to run it");
        return OK;
    }
    println!("{} through {}: {} SQL bytes", if mode == db::Mode::Run { "ran" } else { "queried" }, report.thing, report.bytes);
    match report.result {
        None => println!("done"),
        Some(value) if mode == db::Mode::Query => print_db_rows(&value, args.has("--detail")),
        Some(value) => println!("{}", serde_json::to_string_pretty(&value).expect("db result serialises")),
    }
    OK
}

fn summarise_db_json(value: &mut serde_json::Value, detail: bool) {
    let Some(result) = value.get_mut("result").and_then(serde_json::Value::as_object_mut) else { return };
    let total = result.get("rows").and_then(serde_json::Value::as_array).map(Vec::len).unwrap_or(0);
    let columns: Vec<String> = result.get("dataShape")
        .and_then(|shape| shape.get("fieldDefinitions"))
        .and_then(serde_json::Value::as_object)
        .map(|fields| fields.keys().cloned().collect())
        .or_else(|| result.get("rows").and_then(serde_json::Value::as_array).and_then(|rows| rows.first())
            .and_then(serde_json::Value::as_object).map(|row| row.keys().cloned().collect()))
        .unwrap_or_default();
    if !detail {
        if let Some(rows) = result.get_mut("rows").and_then(serde_json::Value::as_array_mut) { rows.truncate(20); }
    }
    result.insert("total_rows".to_string(), serde_json::json!(total));
    result.insert("columns".to_string(), serde_json::json!(columns));
}

fn print_db_rows(value: &serde_json::Value, detail: bool) {
    let rows = value.get("rows").and_then(serde_json::Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
    let columns: Vec<&str> = value.pointer("/dataShape/fieldDefinitions")
        .and_then(serde_json::Value::as_object)
        .map(|fields| fields.keys().map(String::as_str).collect())
        .or_else(|| rows.first().and_then(serde_json::Value::as_object).map(|row| row.keys().map(String::as_str).collect()))
        .unwrap_or_default();
    println!("{} column(s): {}", columns.len(), columns.join(", "));
    println!("{} row(s)", rows.len());
    let shown = if detail { rows.len() } else { rows.len().min(20) };
    for row in &rows[..shown] {
        println!("{}", serde_json::to_string(row).expect("db row serialises"));
    }
    if shown < rows.len() { println!("... {} more; pass --detail for all", rows.len() - shown); }
}

fn same_path(left: &Path, right: &Path) -> bool {
    fn comparable(path: &Path) -> String {
        let absolute = std::fs::canonicalize(path).unwrap_or_else(|_| {
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(path)
            }
        });
        let text = absolute.to_string_lossy();
        if cfg!(windows) { text.to_ascii_lowercase() } else { text.into_owned() }
    }
    comparable(left) == comparable(right)
}

fn extract(solution: &Solution, args: &Args) -> u8 {
    let (chosen, unreadable) = match targets(solution, args) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("twaco: {e}");
            return FAILED;
        }
    };
    let outcome = workflow::extract(solution, &chosen, &unreadable, !args.has("--all"));
    print_log(&outcome.log);
    print_types_refresh(&outcome.types);
    // "part" rather than "service": an entity yields services or fields depending on what
    // it is, and one counter covers both.
    println!("{} part(s) from {} entity file(s)", outcome.written, outcome.entities);
    if outcome.failed > 0 {
        eprintln!("twaco: {} file(s) failed", outcome.failed);
        return FAILED;
    }
    OK
}


fn sync_cmd(solution: &Solution, args: &Args) -> u8 {
    let check = args.has("--check");
    let (chosen, unreadable) = match targets(solution, args) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("twaco: {e}");
            return FAILED;
        }
    };
    let options = workflow::SyncOptions {
        check,
        allow_structural: args.has("--allow-add-remove"),
        relayout: args.has("--relayout"),
        named: !args.has("--all"),
    };
    let outcome = workflow::sync(solution, &chosen, &unreadable, options);
    print_log(&outcome.log);
    print_types_refresh(&outcome.types);

    if outcome.failed > 0 {
        eprintln!("twaco: {} file(s) failed", outcome.failed);
        return FAILED;
    }
    if outcome.changed == 0 {
        println!("{} entity file(s) already in sync", outcome.checked);
        return OK;
    }
    if check {
        println!("{} of {} entity file(s) would change", outcome.changed, outcome.checked);
        DRIFT
    } else {
        println!("{} of {} entity file(s) updated", outcome.changed, outcome.checked);
        OK
    }
}

/// A workflow log as the CLI shows it: changes on stdout, errors on stderr, in order.
fn print_log(log: &workflow::Log) {
    for line in &log.lines {
        match line {
            workflow::Line::Change(text) => println!("{text}"),
            workflow::Line::Error(text) => eprintln!("twaco: {text}"),
        }
    }
}

fn print_types_refresh(refresh: &types::Refresh) {
    if let Some(files) = refresh.files_written {
        println!("types: refreshed ({files} files written)");
    }
    if let Some(warning) = &refresh.warning {
        eprintln!("twaco: warning: types: {warning}");
    }
}


fn fmt(solution: &Solution, args: &Args) -> u8 {
    let check = args.has("--check");
    let outcome = workflow::fmt(solution, check);
    if outcome.files == 0 {
        println!("no service scripts under {}", solution.src_root().display());
        return OK;
    }
    print_log(&outcome.log);
    for path in &outcome.changed {
        println!("{} {}", if check { "would reformat" } else { "reformatted" }, path.display());
    }
    if outcome.failed > 0 {
        eprintln!("twaco: {} script(s) failed", outcome.failed);
        return FAILED;
    }
    if outcome.changed.is_empty() {
        println!("{} service script(s) are formatted", outcome.files);
        return OK;
    }
    println!(
        "{} of {} service script(s) {}",
        outcome.changed.len(),
        outcome.files,
        if check { "need formatting" } else { "reformatted" }
    );
    if check {
        DRIFT
    } else {
        OK
    }
}

/// The gate: every built-in check, then every one the solution declares.
fn check(solution: &Solution, args: &Args) -> u8 {
    let mut report = twaco::core::check::run(solution);
    if args.has("--live") || solution.gates.live {
        // Credentials are read only here, so the offline gates never need a profile.
        let profile_name = args.profile.as_deref().unwrap_or("default");
        let client = profile::load(&solution.root, profile_name)
            .map(server::Client::new)
            .map_err(|error| error.to_string());
        let checker = client
            .as_ref()
            .map(|client| client as &dyn twaco::core::check::ScriptChecker)
            .map_err(Clone::clone);
        report.gates.push(twaco::core::check::live_parse(solution, checker));
    }
    // Summary by default, detail on request.
    let detail = args.has("--detail");

    for gate in &report.gates {
        match &gate.broken {
            Some(why) => println!("  BROKEN  {:<14} {why}", gate.name),
            None if gate.findings.is_empty() => {
                println!("  ok      {:<14} {} examined", gate.name, gate.examined)
            }
            // A check that reports without blocking says so, rather than reading as a failure
            // someone has to chase.
            None => println!(
                "  {:<7} {:<14} {} finding(s) in {} examined",
                if gate.gates_the_run { "FAIL" } else { "warn" },
                gate.name,
                gate.findings.len(),
                gate.examined
            ),
        }
    }

    if detail {
        for gate in &report.gates {
            for line in &gate.prose {
                println!("    {}: {line}", gate.name);
            }
            for finding in &gate.findings {
                println!("    {finding}");
            }
        }
    }

    println!();
    if report.ok() {
        println!("{} gate(s) passed", report.gates.len());
        return OK;
    }
    println!(
        "{} finding(s) across {} gate(s); {} gate(s) could not run",
        report.findings(),
        report.gates.iter().filter(|g| !g.findings.is_empty()).count(),
        report.broken()
    );
    if !detail {
        println!("run `twaco check --detail` to see them");
    }
    if !report.blocks() {
        // Everything found came from a check that reports without blocking.
        println!("nothing found blocks the run");
        return OK;
    }
    // A gate that could not run is a failure; a gate that ran and found something is drift.
    if report.broken() > 0 {
        FAILED
    } else {
        DRIFT
    }
}

/// Assemble one importable document from the split entity files.
fn bundle(solution: &Solution, args: &Args) -> u8 {
    use twaco::core::bundle;

    let backend_only = args.has("--backend-only");
    let selection =
        if backend_only { bundle::Selection::backend(solution) } else { bundle::Selection::everything() };
    let files = bundle::source_files(solution);
    if files.is_empty() {
        eprintln!("twaco: no entity XML found under {}", solution.root.display());
        return FAILED;
    }

    let built = match bundle::build(&files, &selection) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("twaco: {e}");
            return FAILED;
        }
    };

    // A reference to something this repository owns but this bundle leaves out. Scoped to the
    // selection, so a backend-only build reports what it dropped rather than what it kept.
    for dangling in bundle::dangling_references(&files, &files) {
        println!("  note: {dangling}");
    }

    let name = if backend_only {
        solution.bundle.backend_name.clone()
    } else {
        solution.bundle.name.clone()
    };
    let directory = solution.root.join(&solution.solution.dist);
    let target = directory.join(&name);

    if args.has("--check") {
        // A check compares against what is on disk. Rebuilding and reporting the size would say
        // nothing about whether the bundle anyone is about to import is the current one.
        return match std::fs::read(&target) {
            Ok(existing) if existing == built.bytes => {
                println!(
                    "{} is current: {} entities from {} file(s)",
                    target.display(),
                    built.entities.len(),
                    built.files
                );
                OK
            }
            Ok(_) => {
                println!("{} is out of date; run `twaco bundle`", target.display());
                DRIFT
            }
            Err(_) => {
                println!("{} has not been built; run `twaco bundle`", target.display());
                DRIFT
            }
        };
    }

    if let Err(e) = std::fs::create_dir_all(&directory) {
        eprintln!("twaco: {}: {e}", directory.display());
        return FAILED;
    }
    if let Err(e) = workspace::write_entity(&target, &built.bytes) {
        eprintln!("twaco: {e}");
        return FAILED;
    }
    println!(
        "wrote {}: {} entities from {} file(s), {} bytes",
        target.display(),
        built.entities.len(),
        built.files,
        built.bytes.len()
    );
    OK
}

/// Deploy through import, read-back, configured service calls, and a post-service re-read.
fn deploy_cmd(solution: &Solution, args: &Args) -> u8 {
    if !args.names.is_empty() {
        eprintln!("twaco: deploy takes no positional names; use --only <entity>");
        return FAILED;
    }

    // Offline gates are first, before profiles, bundles, or server traffic.
    if !args.has("--skip-checks") {
        println!("offline gates:");
        let code = check(solution, args);
        if code != OK {
            eprintln!("twaco: offline gates block deploy");
            return code;
        }
        println!();
    }

    let (projects, notes) = match deploy::plan_bundles(
        solution,
        deploy::PlanOptions {
            only_projects: &args.only_projects,
            only: &args.only,
            backend_only: args.has("--backend-only"),
        },
    ) {
        Ok(planned) => planned,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    for note in &notes {
        println!("{note}");
    }

    let profile_name = args.profile.as_deref().unwrap_or("default");
    let profile = match profile::load(&solution.root, profile_name) {
        Ok(profile) => profile,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let apply = args.has("--apply");
    let force = args.has("--force");
    let client = server::Client::new(profile.clone());
    if apply && force && !args.has("--no-backup") {
        match backup::before_forced_deploy(&client, solution, &projects, &backup::new_stamp()) {
            Ok(Some(dir)) => println!("the server's copies of the entities --force overwrites were saved to {dir}"),
            Ok(None) => {}
            Err(error) => {
                eprintln!("twaco: {error} (--no-backup deploys without one)");
                return FAILED;
            }
        }
    }
    let result = deploy::run(
        &client,
        &deploy::DiskBaseline::new(&solution.root),
        &profile,
        &projects,
        apply,
        force,
        !args.only.is_empty(),
    );
    match result {
        Ok(report) => {
            print_deploy_report(&report, apply, force);
            OK
        }
        Err(deploy::DeployError::ParseFailed(failures)) => {
            for failure in failures {
                eprintln!(
                    "twaco: {}/{} {}:{} {}",
                    failure.entity, failure.service, failure.line, failure.column, failure.message
                );
            }
            eprintln!("twaco: live parse failed; no import was sent");
            FAILED
        }
        Err(deploy::DeployError::Conflicts(conflicts)) => {
            for conflict in conflicts {
                let deploy::EntityPlan { collection, name, decision, .. } = conflict;
                if let push::Decision::Refuse(reason) = decision {
                    eprintln!("twaco: {collection}/{name}: refused: {reason}");
                }
            }
            eprintln!("twaco: nothing was imported; pass --force to overwrite these changes");
            DRIFT
        }
        Err(deploy::DeployError::NotKept(report)) => {
            print_deploy_report(&report, true, force);
            for item in &report.not_kept {
                eprintln!(
                    "twaco: {}/{}: not kept (sent {}, read back {}{})",
                    item.collection,
                    item.name,
                    item.sent,
                    item.read_back.as_deref().unwrap_or("nothing"),
                    item.error.as_ref().map(|why| format!("; {why}")).unwrap_or_default()
                );
            }
            FAILED
        }
        Err(error) => {
            eprintln!("twaco: {error}");
            FAILED
        }
    }
}

fn print_deploy_report(report: &deploy::Report, apply: bool, force: bool) {
    println!("live parse: {} script service(s) passed", report.scripts_checked);
    for plan in &report.plans {
        let label = format!("{}/{}", plan.collection, plan.name);
        match &plan.decision {
            push::Decision::AlreadyThere => println!("  {label}: nothing to push"),
            push::Decision::Create => println!("  {label}: would create"),
            push::Decision::Update => println!("  {label}: would update; server unchanged since baseline"),
            push::Decision::Refuse(reason) if force => {
                println!("  {label}: would overwrite with --force ({reason})")
            }
            push::Decision::Refuse(reason) => println!("  {label}: would refuse ({reason})"),
        }
    }
    if !apply {
        for project in &report.projects {
            println!("project {project}: would import one bundle");
        }
        print_deploy_calls(report, false);
        println!("dry run: no import was sent and no baseline was written; pass --apply to deploy");
    } else {
        for project in &report.imported {
            println!("project {project}: imported");
        }
        println!(
            "import read-back: {} matching, {} not kept",
            report.kept.len(),
            report.not_kept.len()
        );
        print_deploy_calls(report, true);
        for (collection, name) in &report.changed_by_deploy {
            println!("changed by the deploy step: {collection}/{name}");
        }
        println!("baseline written once");
    }
}

fn print_deploy_calls(report: &deploy::Report, apply: bool) {
    for planned in &report.calls {
        if planned.skipped {
            println!(
                "project {}: post-import {} skipped because --only was used",
                planned.project, planned.call
            );
        } else if apply {
            println!("project {}: called {}", planned.project, planned.call);
        } else {
            println!("project {}: would call {}", planned.project, planned.call);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The usage text of one command: every block whose first line names it, with the lines
    /// indented under it. `scripts/commands_doc.py` groups the text the same way.
    fn usage_of(command: &str) -> String {
        let mut text = String::new();
        let mut current = false;
        for line in USAGE.lines().skip(2) {
            if line.starts_with("  ") && !line.starts_with("    ") {
                let words: Vec<&str> = line.split_whitespace().collect();
                let key = if matches!(words[0], "entity" | "rename" | "db" | "datatable" | "move" | "copy" | "new") {
                    format!("{} {}", words[0], words[1])
                } else {
                    words[0].to_string()
                };
                current = key == command;
            }
            if current {
                text.push_str(line);
                text.push('\n');
            }
        }
        text
    }

    /// Whether `text` names `flag` as a whole word, so `--only` is not found inside
    /// `--only-projects`.
    fn names(text: &str, flag: &str) -> bool {
        text.match_indices(flag).any(|(at, _)| {
            !text[at + flag.len()..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '-')
        })
    }

    /// `--profile` and `--project` are described once, for every command that takes them.
    fn missing(command: &str, known: &[&str]) -> Vec<String> {
        let own = usage_of(command);
        known
            .iter()
            .filter(|flag| {
                let shared = matches!(**flag, "--profile" | "--project");
                !names(&own, flag) && !(shared && names(USAGE, flag))
            })
            .map(|flag| format!("{command} {flag}"))
            .collect()
    }

    #[test]
    fn usage_names_every_flag_each_command_accepts() {
        let commands = [
            "projects", "types", "extract", "sync", "fmt", "check", "bundle", "deploy", "call", "ext",
            "settings", "catalog", "package", "import", "export", "repo", "logs", "adopt", "rename entity", "rename prefix", "rename field", "rename service", "rename param", "rename table", "rename property", "move service", "move property", "copy service", "copy property", "retemplate", "new building-block", "config-table",
            "entity get", "entity push", "entity delete", "entity carry", "entity restore", "entity status", "db run", "db query", "db clean", "datatable copy",
        ];
        let mut absent = Vec::new();
        for command in commands {
            let args: Vec<String> = command.split(' ').map(str::to_string).collect();
            let (_, _, known) = route(&args).unwrap_or_else(|why| panic!("{command}: {why}"));
            assert!(!usage_of(command).is_empty(), "{command} has no usage block");
            absent.extend(missing(command, known));
        }
        for (command, known) in [("help", HELP_FLAGS), ("guide", GUIDE_FLAGS), ("javadoc", JAVADOC_FLAGS)] {
            absent.extend(missing(command, known));
        }
        assert!(absent.is_empty(), "flags accepted but not in the command's usage: {absent:?}");
    }

    #[test]
    fn an_unknown_command_has_no_route() {
        assert!(route(&["nonsense".to_string()]).is_err());
        assert!(route(&["entity".to_string()]).is_err());
        assert!(route(&["entity".to_string(), "frob".to_string()]).is_err());
        assert!(route(&["rename".to_string()]).is_err());
        assert!(route(&["rename".to_string(), "frob".to_string()]).is_err());
    }

    #[test]
    fn entity_delete_takes_no_generic_workspace_lock() {
        let args = Args::parse(&["Things/T".to_string(), "--apply".to_string()], &["--apply"]).unwrap();
        assert!(!writes_workspace("entity delete", &args), "only a pending rename ledger is a workspace write");
    }

    #[test]
    fn entity_delete_acknowledgement_flags_are_scoped_to_entity_delete() {
        let delete = ["entity".to_string(), "delete".to_string()];
        let (_, _, flags) = route(&delete).unwrap();
        for flag in [
            "--allow-repository-defined",
            "--allow-outside-dependents",
            "--allow-file-repository-data-loss",
        ] {
            assert!(Args::parse(&["Things/T".to_string(), flag.to_string()], flags).is_ok(), "{flag}");
        }
        let push = ["entity".to_string(), "push".to_string()];
        let (_, _, flags) = route(&push).unwrap();
        for flag in [
            "--allow-repository-defined",
            "--allow-outside-dependents",
            "--allow-file-repository-data-loss",
        ] {
            assert!(Args::parse(&["Things/T".to_string(), flag.to_string()], flags).is_err(), "{flag}");
        }
    }

    #[test]
    fn entity_delete_force_deprecation_text_is_stable() {
        assert_eq!(
            entity_delete_force_deprecation(),
            "--force is deprecated for entity delete; it now means --allow-repository-defined --allow-outside-dependents and never covers FileRepository data loss"
        );
    }

    #[test]
    fn entity_carry_takes_no_generic_workspace_lock() {
        let args = Args::parse(&["--renamed".to_string(), "--apply".to_string()], &["--apply", "--renamed"]).unwrap();
        assert!(!writes_workspace("entity carry", &args), "the command takes its own lock, only when it marks the ledger");
    }

    #[test]
    fn db_run_apply_takes_no_workspace_lock() {
        let args = Args::parse(&["migration.sql".to_string(), "--apply".to_string()], &["--apply"]).unwrap();
        assert!(!writes_workspace("db run", &args));
    }
}
