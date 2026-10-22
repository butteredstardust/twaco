use super::flags::{Apply, Detail, Json, Profile, Timeout};
use super::style;
use crate::{print_notices, DRIFT, FAILED, OK};
use std::path::PathBuf;
use std::time::Duration;
use twaco::core::commands::{self, Mode};
use twaco::core::config::Solution;
use twaco::core::{config_table, datatable_copy, db, profile, server, workspace};

/// `twaco config-table <thing> <table> [--backup <file>|--restore <file>|--diff]`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct ConfigTable {
    #[arg(long = "backup", value_name = "file", value_parser = super::flags::path())]
    pub(crate) backup: Option<PathBuf>,
    #[arg(long = "restore", value_name = "file", value_parser = super::flags::path())]
    pub(crate) restore: Option<PathBuf>,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[arg(long = "diff")]
    pub(crate) diff: bool,
    #[command(flatten)]
    pub(crate) detail: Detail,
    #[command(flatten)]
    pub(crate) profile: Profile,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// Read, back up, restore or diff one Thing's configuration table on the server.
///
/// `--restore` is a plan unless `--apply`, as every command that writes to a server is. It
/// refuses a backup made from another Thing or table, and reads the table back afterwards.
pub(crate) fn config_table(solution: &Solution, args: &ConfigTable) -> u8 {
    let listing = args.operands.len() == 1;
    if !listing && args.operands.len() != 2 {
        eprintln!("{} config-table needs <thing> [<table>]", style::prefix());
        return FAILED;
    }
    if listing && (args.backup.is_some() || args.restore.is_some() || args.diff) {
        eprintln!(
            "{} --backup, --restore and --diff need a <table>; `twaco config-table {}` lists them",
            style::prefix(),
            args.operands[0]
        );
        return FAILED;
    }
    let modes = [args.backup.is_some(), args.restore.is_some(), args.diff];
    if modes.iter().filter(|m| **m).count() > 1 {
        eprintln!(
            "{} --backup, --restore and --diff are separate actions; pass one",
            style::prefix()
        );
        return FAILED;
    }
    if args.apply.apply && args.restore.is_none() {
        eprintln!(
            "{} --apply only means something with --restore",
            style::prefix()
        );
        return FAILED;
    }
    // A Thing in the solution may be named by its last segment, as elsewhere. One that is not in
    // the solution is taken as given, except by --diff, which needs the repository's copy.
    let found = workspace::discover(solution).entities;
    let (thing, entity_file) = match workspace::resolve(&found, &args.operands[0]) {
        Ok(entity) if entity.info.collection == "Things" => {
            (entity.info.name.clone(), Some(entity.path.clone()))
        }
        Ok(entity) => {
            eprintln!(
                "{} {} is a {}, and only a Thing has configuration tables here",
                style::prefix(),
                entity.info.name,
                entity.info.collection
            );
            return FAILED;
        }
        Err(workspace::WorkspaceError::UnknownEntity { .. }) if !args.diff => {
            (args.operands[0].clone(), None)
        }
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    let table = args.operands.get(1).cloned().unwrap_or_default();
    let label = if listing {
        thing.clone()
    } else {
        format!("{thing}.{table}")
    };
    let action = if listing {
        commands::config_table::ConfigTableAction::List
    } else if let Some(path) = &args.restore {
        commands::config_table::ConfigTableAction::Restore {
            path: path.clone(),
            mode: if args.apply.apply {
                Mode::Apply
            } else {
                Mode::Plan
            },
        }
    } else if let Some(path) = &args.backup {
        commands::config_table::ConfigTableAction::Backup { path: path.clone() }
    } else if args.diff {
        commands::config_table::ConfigTableAction::Diff {
            entity: entity_file.expect("--diff resolved the Thing in the solution"),
        }
    } else {
        commands::config_table::ConfigTableAction::Read
    };
    let request = commands::config_table::ConfigTableRequest {
        thing,
        table,
        action,
        profile: args.profile.name().to_string(),
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::config_table::execute(
        solution,
        &request,
        server::Client::new,
        &mut notices,
    ) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            if matches!(
                error,
                commands::config_table::ConfigTableCommandError::Backup(_)
            ) {
                eprintln!("{} {error}", style::prefix());
            } else {
                eprintln!("{} {label}: {error}", style::prefix());
            }
            return FAILED;
        }
    };
    print_notices(&notices);
    match outcome {
        commands::config_table::ConfigTableOutcome::Listed { tables, .. } => {
            if tables.is_empty() {
                println!("{label} has no configuration tables");
            }
            for (name, description) in &tables {
                if description.is_empty() {
                    println!("{name}");
                } else {
                    println!("{name}  {description}");
                }
            }
            if !tables.is_empty() {
                println!("read one: twaco config-table {label} \"<table>\"");
            }
            OK
        }
        commands::config_table::ConfigTableOutcome::Restored { plan, .. } => {
            let apply = args.apply.apply;
            match Ok::<_, config_table::TableError>(plan) {
                Ok(plan) if plan.writes == 0 && plan.deletes.is_empty() => {
                    println!("{label}: already matches the backup; nothing to restore");
                    OK
                }
                Ok(plan) => {
                    let (verb, removal) = if apply {
                        ("restored", "removed")
                    } else {
                        ("would restore", "remove")
                    };
                    println!(
                        "{label}: {verb} {} row(s) and {removal} {} added since the backup{}",
                        plan.writes,
                        plan.deletes.len(),
                        if plan.deletes.is_empty() {
                            String::new()
                        } else {
                            format!(" ({})", plan.deletes.join(", "))
                        }
                    );
                    if apply {
                        println!("read back and matching the backup");
                    } else {
                        println!("dry run: nothing was written; pass --apply to restore");
                    }
                    OK
                }
                Err(_) => unreachable!(),
            }
        }
        commands::config_table::ConfigTableOutcome::BackedUp { table: live, .. } => {
            println!(
                "backed up {} row(s) of {label} to {}",
                live.rows.len(),
                args.backup
                    .as_ref()
                    .expect("backup action has a path")
                    .display()
            );
            OK
        }
        commands::config_table::ConfigTableOutcome::Diffed {
            table: live,
            differences: found,
            ..
        } => {
            if found.is_empty() {
                println!(
                    "{label}: {} row(s), identical to source control",
                    live.rows.len()
                );
                return OK;
            }
            println!("{label}: {} difference(s) from source control", found.len());
            for line in found {
                println!("  {line}");
            }
            DRIFT
        }
        commands::config_table::ConfigTableOutcome::Read { table: live, .. } => {
            let key = config_table::primary_key(&live.data_shape);
            println!(
                "{label}: {} row(s); primary key {}",
                live.rows.len(),
                if key.is_empty() {
                    "none".to_string()
                } else {
                    key.join(", ")
                }
            );
            if args.detail.detail {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&live.rows).expect("JSON values serialise")
                );
            } else if let Some(first) = live.rows.first() {
                println!(
                    "first row: {}",
                    serde_json::to_string(first).expect("JSON values serialise")
                );
            }
            OK
        }
    }
}

/// `twaco call <target> <service> [<json>] [--timeout <seconds>] [--detail] [--as <test user>]`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Call {
    #[command(flatten)]
    pub(crate) timeout: Timeout,
    #[command(flatten)]
    pub(crate) detail: Detail,
    #[command(flatten)]
    pub(crate) profile: Profile,
    #[arg(long = "with-logs")]
    pub(crate) with_logs: bool,
    #[arg(long = "as", value_name = "new")]
    pub(crate) user: Option<String>,
    #[arg(long = "lang", value_name = "code")]
    pub(crate) lang: Option<String>,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// Execute exactly the named opaque service. There is no dry run because twaco cannot infer
/// whether an arbitrary ThingWorx service writes. The MCP surface must decide separately
/// whether and how to expose this capability.
pub(crate) fn call(solution: &Solution, args: &Call) -> u8 {
    if !(2..=3).contains(&args.operands.len()) {
        eprintln!(
            "{} call needs <target> <service> and an optional JSON object",
            style::prefix()
        );
        return FAILED;
    }
    let parameters = match args.operands.get(2) {
        Some(text) => match serde_json::from_str::<serde_json::Value>(text) {
            Ok(value) if value.is_object() => value,
            Ok(_) => {
                eprintln!("{} call parameters must be a JSON object", style::prefix());
                return FAILED;
            }
            Err(error) => {
                eprintln!(
                    "{} call parameters are not valid JSON: {error}",
                    style::prefix()
                );
                return FAILED;
            }
        },
        None => serde_json::json!({}),
    };
    let request = commands::call::CallRequest {
        target: args.operands[0].clone(),
        service: args.operands[1].clone(),
        parameters,
        timeout: args.timeout.or(Duration::from_secs(120)),
        mode: Mode::Apply,
        profile: args.profile.name().to_string(),
        with_logs: args.with_logs,
        profile_before_target: true,
        as_user: args.user.clone(),
        lang: args.lang.clone(),
    };
    let mut notices = commands::Notices::default();
    let outcome =
        match commands::call::execute(solution, &request, server::Client::new, &mut notices) {
            Ok(outcome) => outcome,
            Err(error) => {
                print_notices(&notices);
                if let commands::call::CallCommandError::Call {
                    target,
                    error: call_error,
                    logs,
                } = &error
                {
                    if let Some(logged) = &**logs {
                        if target.to_string() != args.operands[0] {
                            eprintln!("{} calling {target}", style::prefix());
                        }
                        use twaco::core::logs;
                        match logged {
                            Ok(entries) => {
                                println!(
                                    "--- logged during the call: {} entr{}",
                                    entries.len(),
                                    if entries.len() == 1 { "y" } else { "ies" }
                                );
                                for (log, entry) in entries {
                                    println!("{log}: {}", logs::line(entry));
                                }
                                println!("---");
                            }
                            Err(log_error) => {
                                eprintln!(
                                    "{} the call's logs could not be read: {log_error}",
                                    style::prefix()
                                )
                            }
                        }
                        eprintln!("{} {call_error}", style::prefix());
                        return FAILED;
                    }
                }
                eprintln!("{} {error}", style::prefix());
                return FAILED;
            }
        };
    print_notices(&notices);
    let commands::call::CallOutcome::Applied {
        target,
        reply,
        logs,
        ..
    } = outcome
    else {
        unreachable!("the command line always calls")
    };
    if target.to_string() != args.operands[0] {
        eprintln!("{} calling {target}", style::prefix());
    }
    if let Some(logged) = logs {
        use twaco::core::logs;
        match logged {
            Ok(entries) => {
                println!(
                    "--- logged during the call: {} entr{}",
                    entries.len(),
                    if entries.len() == 1 { "y" } else { "ies" }
                );
                for (log, entry) in &entries {
                    println!("{log}: {}", logs::line(entry));
                }
                println!("---");
            }
            Err(error) => eprintln!(
                "{} the call's logs could not be read: {error}",
                style::prefix()
            ),
        }
    }
    match reply {
        None => println!("done"),
        Some(value) if args.detail.detail || !is_info_table(&value) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&value).expect("JSON value serialises")
            );
        }
        Some(value) => print_info_table_summary(&value),
    }
    OK
}

/// `twaco logs <log> [...]` and `twaco logs level <log> [<LEVEL>] [...]`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Logs {
    #[arg(long = "since", value_name = "duration")]
    pub(crate) since: Option<String>,
    #[arg(long = "from", value_name = "address")]
    pub(crate) from: Option<String>,
    #[arg(long = "to", value_name = "address")]
    pub(crate) to: Option<String>,
    #[arg(long = "level", value_name = "level")]
    pub(crate) level: Option<String>,
    #[arg(long = "grep", value_name = "text")]
    pub(crate) grep: Option<String>,
    #[arg(long = "regex", value_name = "re")]
    pub(crate) regex: Option<String>,
    #[arg(long = "user", value_name = "value")]
    pub(crate) user: Option<String>,
    #[arg(long = "thread", value_name = "value")]
    pub(crate) thread: Option<String>,
    #[arg(long = "origin", value_name = "value")]
    pub(crate) origin: Option<String>,
    #[arg(long = "limit", value_name = "n")]
    pub(crate) limit: Option<String>,
    #[arg(long = "oldest-first")]
    pub(crate) oldest_first: bool,
    #[command(flatten)]
    pub(crate) json: Json,
    #[command(flatten)]
    pub(crate) profile: Profile,
    #[arg(long = "sublogger", value_name = "name")]
    pub(crate) sublogger: Option<String>,
    #[arg(long = "reset")]
    pub(crate) reset: bool,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// Read one of the server's logs. Read-only, so it takes no lock.
pub(crate) fn logs_cmd(solution: &Solution, args: &Logs) -> u8 {
    use twaco::core::logs;
    if args.operands.first().map(String::as_str) == Some("level") {
        return log_level_cmd(solution, args);
    }
    for (only_for_levels, given) in [
        ("--sublogger", args.sublogger.is_some()),
        ("--reset", args.reset),
        ("--apply", args.apply.apply),
    ] {
        if given {
            eprintln!(
                "{} logs: {only_for_levels} belongs to `twaco logs level`",
                style::prefix()
            );
            return FAILED;
        }
    }
    let built = (|| -> Result<logs::Query, String> {
        let [log] = args.operands.as_slice() else {
            return Err(format!(
                "logs needs one log name: {}",
                logs::LOGS.join(", ")
            ));
        };
        let now = logs::now_ms();
        let (from_ms, to_ms) = match (
            args.since.as_deref(),
            args.from.as_deref(),
            args.to.as_deref(),
        ) {
            (Some(_), Some(_), _) | (Some(_), _, Some(_)) => {
                return Err(
                    "--since is a window ending now; give it, or --from and --to, not both"
                        .to_string(),
                )
            }
            (since, None, None) => (
                now - logs::parse_since(since.unwrap_or("1h")).map_err(|e| e.to_string())?,
                now,
            ),
            (None, from, to) => {
                let to_ms =
                    logs::parse_time(to.unwrap_or("now"), now).map_err(|e| e.to_string())?;
                let from_ms = match from {
                    Some(from) => logs::parse_time(from, now).map_err(|e| e.to_string())?,
                    None => to_ms - 3_600_000,
                };
                (from_ms, to_ms)
            }
        };
        let search = match (args.grep.as_deref(), args.regex.as_deref()) {
            (Some(_), Some(_)) => {
                return Err("--grep and --regex are two ways to search; give one".to_string())
            }
            (Some(text), None) => Some(logs::Search::Grep(text.to_string())),
            (None, Some(expression)) => Some(logs::Search::Regex(expression.to_string())),
            (None, None) => None,
        };
        let limit = match args.limit.as_deref() {
            Some(text) => text
                .parse::<u64>()
                .map_err(|_| format!("--limit needs a whole number, not {text:?}"))?,
            None => 100,
        };
        Ok(logs::Query {
            log: log.clone(),
            from_ms,
            to_ms,
            level: args
                .level
                .as_deref()
                .map(logs::level)
                .transpose()
                .map_err(|e| e.to_string())?,
            search,
            user: args.user.as_deref().map(str::to_string),
            thread: args.thread.as_deref().map(str::to_string),
            origin: args.origin.as_deref().map(str::to_string),
            limit,
            oldest_first: args.oldest_first,
        })
    })();
    let query = match built {
        Ok(query) => query,
        Err(why) => {
            eprintln!("{} logs: {why}", style::prefix());
            return FAILED;
        }
    };
    let profile_name = args.profile.name();
    let client = match profile::load(&solution.root, profile_name) {
        Ok(profile) => server::Client::new(profile),
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    let outcome = match logs::query(&client, &query) {
        Ok(outcome) => outcome,
        Err(error) => {
            eprintln!("{} logs: {error}", style::prefix());
            // A name the platform does not ship is the likely cause of a missing log.
            if error.to_string().contains("404") && !logs::LOGS.contains(&query.log.as_str()) {
                eprintln!(
                    "{} the platform's logs are {}",
                    style::prefix(),
                    logs::LOGS.join(", ")
                );
            }
            return FAILED;
        }
    };
    for entry in &outcome.entries {
        if args.json.json {
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
    if args.json.json {
        eprintln!("{summary}");
    } else {
        println!("{summary}");
    }
    OK
}

/// `twaco logs level <log> [<LEVEL>] [--sublogger S] [--reset] [--apply]`. Reading is the default;
/// a change is a plan unless --apply, as for every server write. It writes no workspace file.
fn log_level_cmd(solution: &Solution, args: &Logs) -> u8 {
    use twaco::core::logs;
    let request = (|| -> Result<(String, Option<logs::Change>), String> {
        let rest = &args.operands[1..];
        let sublogger = args.sublogger.clone();
        let (log, level) = match rest {
            [log] => (log.clone(), None),
            [log, level] => (
                log.clone(),
                Some(logs::level(level).map_err(|e| e.to_string())?),
            ),
            _ => return Err("logs level needs a log name and, to change it, a level".to_string()),
        };
        let change = match (level, args.reset) {
            (Some(_), true) => return Err("give a level or --reset, not both".to_string()),
            (Some(level), false) => Some(logs::Change::Set { level, sublogger }),
            (None, true) => Some(logs::Change::Reset { sublogger }),
            (None, false) if sublogger.is_some() => {
                return Err("--sublogger needs a level to set, or --reset".to_string())
            }
            (None, false) => None,
        };
        if change.is_none() && args.apply.apply {
            return Err("--apply needs a level to set, or --reset".to_string());
        }
        Ok((log, change))
    })();
    let (log, change) = match request {
        Ok(request) => request,
        Err(why) => {
            eprintln!("{} logs level: {why}", style::prefix());
            return FAILED;
        }
    };
    let request = commands::logs::LogLevelRequest {
        log: log.clone(),
        change,
        mode: if args.apply.apply {
            Mode::Apply
        } else {
            Mode::Plan
        },
        profile: args.profile.name().to_string(),
    };
    let mut notices = commands::Notices::default();
    let outcome =
        match commands::logs::execute(solution, &request, server::Client::new, &mut notices) {
            Ok(outcome) => outcome,
            Err(error) => {
                print_notices(&notices);
                eprintln!("{} logs level: {error}", style::prefix());
                return FAILED;
            }
        };
    print_notices(&notices);
    match outcome {
        commands::logs::LogLevelOutcome::Levels { levels, .. } => {
            println!("{log}: {}", levels.level);
            for (name, level) in &levels.subloggers {
                println!("  {name}: {level}");
            }
            OK
        }
        commands::logs::LogLevelOutcome::Plan { report, .. }
        | commands::logs::LogLevelOutcome::Applied { report, .. } => {
            let apply = args.apply.apply;
            if apply {
                println!("changed {}", report.plan);
            } else {
                println!(
                    "would change {}; nothing sent (pass --apply; the level is the whole server's)",
                    report.plan
                );
            }
            for undo in &report.undo {
                println!("to put it back: {undo}");
            }
            OK
        }
    }
}

/// `twaco datatable copy`.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum Datatable {
    Copy(DatatableCopy),
}

/// `twaco datatable copy <old> <new> [--map <old=new,...>] ...`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct DatatableCopy {
    #[arg(long = "map", value_name = "value")]
    pub(crate) map: Option<String>,
    #[arg(long = "drop-unmapped")]
    pub(crate) drop_unmapped: bool,
    #[arg(long = "append")]
    pub(crate) append: bool,
    #[arg(long = "max-rows", value_name = "n")]
    pub(crate) max_rows: Option<String>,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) profile: Profile,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// Copy a DataTable's rows into the one that replaced it. A plan reads; an apply writes, then
/// reads the target back and compares.
pub(crate) fn datatable_copy_cmd(solution: &Solution, args: &DatatableCopy) -> u8 {
    let [old, new] = args.operands.as_slice() else {
        eprintln!(
            "{} datatable copy needs <old> <new> DataTable names",
            style::prefix()
        );
        return FAILED;
    };
    let map = match args
        .map
        .as_ref()
        .map(|text| datatable_copy::parse_map(text))
        .transpose()
    {
        Ok(map) => map.unwrap_or_default(),
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    let max_rows = match args.max_rows.as_ref().map(|value| value.parse::<u64>()) {
        None => 100_000,
        Some(Ok(value)) if value > 0 => value,
        Some(_) => {
            eprintln!(
                "{} --max-rows needs a positive whole number",
                style::prefix()
            );
            return FAILED;
        }
    };
    let request = commands::datatable_copy::DataTableCopyRequest {
        old: old.clone(),
        new: new.clone(),
        map,
        drop_unmapped: args.drop_unmapped,
        append: args.append,
        max_rows,
        mode: if args.apply.apply {
            Mode::Apply
        } else {
            Mode::Plan
        },
        profile: args.profile.name().to_string(),
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::datatable_copy::execute(
        solution,
        &request,
        server::Client::new,
        &mut notices,
    ) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    let (report, apply) = match outcome {
        commands::datatable_copy::DataTableCopyOutcome::Plan { report, .. } => (report, false),
        commands::datatable_copy::DataTableCopyOutcome::Applied { report, .. } => (report, true),
    };
    if args.json.json {
        let value = serde_json::to_value(&report).expect("copy report serialises");
        println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("copy report serialises")
        );
        return OK;
    }
    println!(
        "{}: {} -> {}",
        if apply { "applied" } else { "plan" },
        report.old,
        report.new
    );
    println!(
        "  rows: {} to copy, {} already in the target",
        report.source_rows, report.target_rows_before
    );
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
        println!(
            "  wrote {} row(s); read back equal: {}",
            report.written, report.verified
        );
        println!("  not carried: each row's source, tags and timestamp (the write stamps the caller and the time)");
    } else {
        println!("dry run: nothing was written; pass --apply to copy");
    }
    OK
}

/// `twaco db run|query|clean`.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum Db {
    Run(DbRun),
    Query(DbQuery),
    Clean(DbClean),
}

impl Db {
    /// The command's [`USAGE`](super::spec::USAGE) path.
    pub(crate) fn path(&self) -> &'static str {
        match self {
            Db::Run(_) => "db run",
            Db::Query(_) => "db query",
            Db::Clean(_) => "db clean",
        }
    }
}

/// `twaco db run <file.sql> [--thing <name>] [--no-transaction] [--timeout <seconds>]`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct DbRun {
    #[arg(long = "thing", value_name = "name")]
    pub(crate) thing: Option<String>,
    #[arg(long = "no-transaction")]
    pub(crate) no_transaction: bool,
    #[command(flatten)]
    pub(crate) timeout: Timeout,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) profile: Profile,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco db query <file.sql>|-q <sql> [--thing <name>] [--max-rows <n>] [--timeout <seconds>]`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct DbQuery {
    #[arg(short = 'q', value_name = "sql")]
    pub(crate) sql: Option<String>,
    #[arg(long = "thing", value_name = "name")]
    pub(crate) thing: Option<String>,
    #[arg(long = "max-rows", value_name = "n")]
    pub(crate) max_rows: Option<String>,
    #[command(flatten)]
    pub(crate) timeout: Timeout,
    #[command(flatten)]
    pub(crate) profile: Profile,
    #[command(flatten)]
    pub(crate) detail: Detail,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco db clean`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct DbClean {
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) profile: Profile,
    #[command(flatten)]
    pub(crate) json: Json,
}

/// A `db run` or `db query` line as [`db_cmd`] reads it; what one of them does not take reads
/// as not given.
pub(crate) struct DbLine<'a> {
    mode: db::Mode,
    operands: &'a [String],
    sql: Option<&'a String>,
    thing: Option<&'a String>,
    max_rows: Option<&'a String>,
    timeout: &'a Timeout,
    apply: bool,
    no_transaction: bool,
    detail: bool,
    json: bool,
    profile: &'a Profile,
}

impl DbRun {
    /// The line as `db_cmd` reads it.
    pub(crate) fn line(&self) -> DbLine<'_> {
        DbLine {
            mode: db::Mode::Run,
            operands: &self.operands,
            sql: None,
            thing: self.thing.as_ref(),
            max_rows: None,
            timeout: &self.timeout,
            apply: self.apply.apply,
            no_transaction: self.no_transaction,
            detail: false,
            json: self.json.json,
            profile: &self.profile,
        }
    }
}

impl DbQuery {
    /// The line as `db_cmd` reads it.
    pub(crate) fn line(&self) -> DbLine<'_> {
        DbLine {
            mode: db::Mode::Query,
            operands: &self.operands,
            sql: self.sql.as_ref(),
            thing: self.thing.as_ref(),
            max_rows: self.max_rows.as_ref(),
            timeout: &self.timeout,
            apply: false,
            no_transaction: false,
            detail: self.detail.detail,
            json: self.json.json,
            profile: &self.profile,
        }
    }
}

/// Delete the temporary Things an interrupted `db run` or `db query` left on the server. Plans by
/// default; touches only names twaco generates, on `Database` Things.
pub(crate) fn db_clean_cmd(solution: &Solution, args: &DbClean) -> u8 {
    let apply = args.apply.apply;
    let request = commands::db::DbRequest::Clean {
        mode: if apply { Mode::Apply } else { Mode::Plan },
        profile: args.profile.name().to_string(),
    };
    let mut notices = commands::Notices::default();
    let swept = match commands::db::execute(solution, &request, server::Client::new, &mut notices) {
        Ok(commands::db::DbOutcome::Cleaned { things, .. }) => things,
        Ok(commands::db::DbOutcome::Executed { .. }) => unreachable!(),
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    if args.json.json {
        let key = if apply { "applied" } else { "plan" };
        let value = serde_json::json!({ (key): true, "things": swept });
        println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("sweep report serialises")
        );
    } else {
        println!("{}:", if apply { "applied" } else { "plan" });
        for thing in &swept {
            let why = thing
                .why
                .as_ref()
                .map(|why| format!("  ({why})"))
                .unwrap_or_default();
            println!("  {}  {:?}{why}", thing.name, thing.status);
        }
        if swept.is_empty() {
            println!("  no temporary Things on the server");
        } else if !apply {
            println!("dry run: nothing was deleted; pass --apply to delete the stale ones");
        }
    }
    if swept
        .iter()
        .any(|thing| thing.status == db::SweepStatus::Failed)
    {
        FAILED
    } else {
        OK
    }
}

pub(crate) fn db_cmd(solution: &Solution, args: &DbLine) -> u8 {
    let mode = args.mode;
    let inline = args.sql;
    let sql = match (mode, args.operands, inline) {
        (db::Mode::Run, [file], None) | (db::Mode::Query, [file], None) => {
            match std::fs::read_to_string(file) {
                Ok(sql) => sql,
                Err(error) => {
                    eprintln!("{} cannot read SQL file {file}: {error}", style::prefix());
                    return FAILED;
                }
            }
        }
        (db::Mode::Query, [], Some(sql)) => sql.clone(),
        (db::Mode::Run, _, Some(_)) => {
            eprintln!(
                "{} db run needs one <file.sql>; -q belongs to db query",
                style::prefix()
            );
            return FAILED;
        }
        (db::Mode::Query, _, _) => {
            eprintln!(
                "{} db query needs one <file.sql>, or -q <sql>, but not both",
                style::prefix()
            );
            return FAILED;
        }
        _ => {
            eprintln!("{} db run needs one <file.sql>", style::prefix());
            return FAILED;
        }
    };
    let max_rows = match args.max_rows {
        Some(value) => match value.parse::<u64>() {
            Ok(value) if value > 0 => value,
            _ => {
                eprintln!(
                    "{} --max-rows needs a positive whole number",
                    style::prefix()
                );
                return FAILED;
            }
        },
        None => 500,
    };
    let options = db::Options {
        mode,
        thing: args.thing.cloned(),
        apply: mode == db::Mode::Query || args.apply,
        no_transaction: args.no_transaction,
        max_rows,
        timeout: args.timeout.or(Duration::from_secs(120)),
    };
    let request = commands::db::DbRequest::Execute {
        sql,
        options,
        profile: args.profile.name().to_string(),
    };
    let mut notices = commands::Notices::default();
    let report = match commands::db::execute(solution, &request, server::Client::new, &mut notices)
    {
        Ok(commands::db::DbOutcome::Executed { report, .. }) => report,
        Ok(commands::db::DbOutcome::Cleaned { .. }) => unreachable!(),
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    if args.json {
        let mut value = serde_json::to_value(&report).expect("db report serialises");
        if mode == db::Mode::Query {
            summarise_db_json(&mut value, args.detail);
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("db report serialises")
        );
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
    println!(
        "{} through {}: {} SQL bytes",
        if mode == db::Mode::Run {
            "ran"
        } else {
            "queried"
        },
        report.thing,
        report.bytes
    );
    match report.result {
        None => println!("done"),
        Some(value) if mode == db::Mode::Query => print_db_rows(&value, args.detail),
        Some(value) => println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("db result serialises")
        ),
    }
    OK
}

fn summarise_db_json(value: &mut serde_json::Value, detail: bool) {
    let Some(result) = value
        .get_mut("result")
        .and_then(serde_json::Value::as_object_mut)
    else {
        return;
    };
    let total = result
        .get("rows")
        .and_then(serde_json::Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let columns: Vec<String> = result
        .get("dataShape")
        .and_then(|shape| shape.get("fieldDefinitions"))
        .and_then(serde_json::Value::as_object)
        .map(|fields| fields.keys().cloned().collect())
        .or_else(|| {
            result
                .get("rows")
                .and_then(serde_json::Value::as_array)
                .and_then(|rows| rows.first())
                .and_then(serde_json::Value::as_object)
                .map(|row| row.keys().cloned().collect())
        })
        .unwrap_or_default();
    if !detail {
        if let Some(rows) = result
            .get_mut("rows")
            .and_then(serde_json::Value::as_array_mut)
        {
            rows.truncate(20);
        }
    }
    result.insert("total_rows".to_string(), serde_json::json!(total));
    result.insert("columns".to_string(), serde_json::json!(columns));
}

fn print_db_rows(value: &serde_json::Value, detail: bool) {
    let rows = value
        .get("rows")
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let columns: Vec<&str> = value
        .pointer("/dataShape/fieldDefinitions")
        .and_then(serde_json::Value::as_object)
        .map(|fields| fields.keys().map(String::as_str).collect())
        .or_else(|| {
            rows.first()
                .and_then(serde_json::Value::as_object)
                .map(|row| row.keys().map(String::as_str).collect())
        })
        .unwrap_or_default();
    println!("{} column(s): {}", columns.len(), columns.join(", "));
    println!("{} row(s)", rows.len());
    let shown = if detail {
        rows.len()
    } else {
        rows.len().min(20)
    };
    for row in &rows[..shown] {
        println!("{}", serde_json::to_string(row).expect("db row serialises"));
    }
    if shown < rows.len() {
        println!("... {} more; pass --detail for all", rows.len() - shown);
    }
}

fn is_info_table(value: &serde_json::Value) -> bool {
    value.get("dataShape").is_some() && value.get("rows").is_some_and(serde_json::Value::is_array)
}

fn print_info_table_summary(value: &serde_json::Value) {
    let rows = value["rows"]
        .as_array()
        .expect("is_info_table checked rows");
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
        println!(
            "{}",
            serde_json::to_string_pretty(first).expect("JSON value serialises")
        );
    }
}
