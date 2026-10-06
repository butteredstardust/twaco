use super::super::*;

/// Execute exactly the named opaque service. There is no dry run because twaco cannot infer
/// whether an arbitrary ThingWorx service writes. The MCP surface must decide separately
/// whether and how to expose this capability.
/// Read, back up, restore or diff one Thing's configuration table on the server.
///
/// `--restore` is a plan unless `--apply`, as every command that writes to a server is. It
/// refuses a backup made from another Thing or table, and reads the table back afterwards.
pub(crate) fn config_table(solution: &Solution, args: &Args) -> u8 {
    if args.names.len() != 2 {
        eprintln!("twaco: config-table needs <thing> <table>");
        return FAILED;
    }
    let modes = [
        args.backup.is_some(),
        args.restore.is_some(),
        args.has("--diff"),
    ];
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
        Ok(entity) if entity.info.collection == "Things" => {
            (entity.info.name.clone(), Some(entity.path.clone()))
        }
        Ok(entity) => {
            eprintln!(
                "twaco: {} is a {}, and only a Thing has configuration tables here",
                entity.info.name, entity.info.collection
            );
            return FAILED;
        }
        Err(workspace::WorkspaceError::UnknownEntity { .. }) if !args.has("--diff") => {
            (args.names[0].clone(), None)
        }
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let table = &args.names[1];
    let label = format!("{thing}.{table}");
    let action = if let Some(path) = &args.restore {
        commands::config_table::ConfigTableAction::Restore {
            path: path.clone(),
            mode: if args.has("--apply") {
                Mode::Apply
            } else {
                Mode::Plan
            },
        }
    } else if let Some(path) = &args.backup {
        commands::config_table::ConfigTableAction::Backup { path: path.clone() }
    } else if args.has("--diff") {
        commands::config_table::ConfigTableAction::Diff {
            entity: entity_file.expect("--diff resolved the Thing in the solution"),
        }
    } else {
        commands::config_table::ConfigTableAction::Read
    };
    let request = commands::config_table::ConfigTableRequest {
        thing,
        table: table.to_string(),
        action,
        profile: args.profile.as_deref().unwrap_or("default").to_string(),
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
                eprintln!("twaco: {error}");
            } else {
                eprintln!("twaco: {label}: {error}");
            }
            return FAILED;
        }
    };
    print_notices(&notices);
    match outcome {
        commands::config_table::ConfigTableOutcome::Restored { plan, .. } => {
            let apply = args.has("--apply");
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
            if args.has("--detail") {
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

pub(crate) fn call(solution: &Solution, args: &Args) -> u8 {
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
    let request = commands::call::CallRequest {
        target: args.names[0].clone(),
        service: args.names[1].clone(),
        parameters,
        timeout: args.timeout.unwrap_or(Duration::from_secs(120)),
        mode: Mode::Apply,
        profile: args.profile.as_deref().unwrap_or("default").to_string(),
        with_logs: args.has("--with-logs"),
        profile_before_target: true,
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
                        if target.to_string() != args.names[0] {
                            eprintln!("twaco: calling {target}");
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
                                eprintln!("twaco: the call's logs could not be read: {log_error}")
                            }
                        }
                        eprintln!("twaco: {call_error}");
                        return FAILED;
                    }
                }
                eprintln!("twaco: {error}");
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
    if target.to_string() != args.names[0] {
        eprintln!("twaco: calling {target}");
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
            Err(error) => eprintln!("twaco: the call's logs could not be read: {error}"),
        }
    }
    match reply {
        None => println!("done"),
        Some(value) if args.has("--detail") || !is_info_table(&value) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&value).expect("JSON value serialises")
            );
        }
        Some(value) => print_info_table_summary(&value),
    }
    OK
}

/// Read one of the server's logs. Read-only, so it takes no lock.
pub(crate) fn logs_cmd(solution: &Solution, args: &Args) -> u8 {
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
            return Err(format!(
                "logs needs one log name: {}",
                logs::LOGS.join(", ")
            ));
        };
        let now = logs::now_ms();
        let (from_ms, to_ms) = match (value("--since"), value("--from"), value("--to")) {
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
        let search = match (value("--grep"), value("--regex")) {
            (Some(_), Some(_)) => {
                return Err("--grep and --regex are two ways to search; give one".to_string())
            }
            (Some(text), None) => Some(logs::Search::Grep(text.to_string())),
            (None, Some(expression)) => Some(logs::Search::Regex(expression.to_string())),
            (None, None) => None,
        };
        let limit = match value("--limit") {
            Some(text) => text
                .parse::<u64>()
                .map_err(|_| format!("--limit needs a whole number, not {text:?}"))?,
            None => 100,
        };
        Ok(logs::Query {
            log: log.clone(),
            from_ms,
            to_ms,
            level: value("--level")
                .map(logs::level)
                .transpose()
                .map_err(|e| e.to_string())?,
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
            [log, level] => (
                log.clone(),
                Some(logs::level(level).map_err(|e| e.to_string())?),
            ),
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
    let request = commands::logs::LogLevelRequest {
        log: log.clone(),
        change,
        mode: if args.has("--apply") {
            Mode::Apply
        } else {
            Mode::Plan
        },
        profile: args.profile.as_deref().unwrap_or("default").to_string(),
    };
    let mut notices = commands::Notices::default();
    let outcome =
        match commands::logs::execute(solution, &request, server::Client::new, &mut notices) {
            Ok(outcome) => outcome,
            Err(error) => {
                print_notices(&notices);
                eprintln!("twaco: logs level: {error}");
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
            let apply = args.has("--apply");
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

/// Copy a DataTable's rows into the one that replaced it. A plan reads; an apply writes, then
/// reads the target back and compares.
pub(crate) fn datatable_copy_cmd(solution: &Solution, args: &Args) -> u8 {
    let [old, new] = args.names.as_slice() else {
        eprintln!("twaco: datatable copy needs <old> <new> DataTable names");
        return FAILED;
    };
    let map = match args
        .values
        .get("--map")
        .map(|text| datatable_copy::parse_map(text))
        .transpose()
    {
        Ok(map) => map.unwrap_or_default(),
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let max_rows = match args
        .values
        .get("--max-rows")
        .map(|value| value.parse::<u64>())
    {
        None => 100_000,
        Some(Ok(value)) if value > 0 => value,
        Some(_) => {
            eprintln!("twaco: --max-rows needs a positive whole number");
            return FAILED;
        }
    };
    let request = commands::datatable_copy::DataTableCopyRequest {
        old: old.clone(),
        new: new.clone(),
        map,
        drop_unmapped: args.has("--drop-unmapped"),
        append: args.has("--append"),
        max_rows,
        mode: if args.has("--apply") {
            Mode::Apply
        } else {
            Mode::Plan
        },
        profile: args
            .profile
            .clone()
            .unwrap_or_else(|| "default".to_string()),
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
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    print_notices(&notices);
    let (report, apply) = match outcome {
        commands::datatable_copy::DataTableCopyOutcome::Plan { report, .. } => (report, false),
        commands::datatable_copy::DataTableCopyOutcome::Applied { report, .. } => (report, true),
    };
    if args.has("--json") {
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

/// Delete the temporary Things an interrupted `db run` or `db query` left on the server. Plans by
/// default; touches only names twaco generates, on `Database` Things.
pub(crate) fn db_clean_cmd(solution: &Solution, args: &Args) -> u8 {
    let apply = args.has("--apply");
    let request = commands::db::DbRequest::Clean {
        mode: if apply { Mode::Apply } else { Mode::Plan },
        profile: args.profile.as_deref().unwrap_or("default").to_string(),
    };
    let mut notices = commands::Notices::default();
    let swept = match commands::db::execute(solution, &request, server::Client::new, &mut notices) {
        Ok(commands::db::DbOutcome::Cleaned { things, .. }) => things,
        Ok(commands::db::DbOutcome::Executed { .. }) => unreachable!(),
        Err(error) => {
            print_notices(&notices);
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    print_notices(&notices);
    if args.has("--json") {
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

pub(crate) fn db_cmd(solution: &Solution, route: &str, args: &Args) -> u8 {
    let mode = if route == "db run" {
        db::Mode::Run
    } else {
        db::Mode::Query
    };
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
    let options = db::Options {
        mode,
        thing: args.values.get("--thing").cloned(),
        apply: mode == db::Mode::Query || args.has("--apply"),
        no_transaction: args.has("--no-transaction"),
        max_rows,
        timeout: args.timeout.unwrap_or(Duration::from_secs(120)),
    };
    let request = commands::db::DbRequest::Execute {
        sql,
        options,
        profile: args.profile.as_deref().unwrap_or("default").to_string(),
    };
    let mut notices = commands::Notices::default();
    let report = match commands::db::execute(solution, &request, server::Client::new, &mut notices)
    {
        Ok(commands::db::DbOutcome::Executed { report, .. }) => report,
        Ok(commands::db::DbOutcome::Cleaned { .. }) => unreachable!(),
        Err(error) => {
            print_notices(&notices);
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    print_notices(&notices);
    if args.has("--json") {
        let mut value = serde_json::to_value(&report).expect("db report serialises");
        if mode == db::Mode::Query {
            summarise_db_json(&mut value, args.has("--detail"));
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
        Some(value) if mode == db::Mode::Query => print_db_rows(&value, args.has("--detail")),
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
