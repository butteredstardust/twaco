use super::*;

pub(crate) fn datatable_copy_tool(
    solution: &Solution,
    arguments: &Value,
) -> Result<Value, ToolError> {
    let dry_run = flag(arguments, "dry_run", true);
    let name = |key: &str| {
        text(arguments, key)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .ok_or_else(|| ToolError::invalid(format!("`{key}` is required")))
    };
    let mut map = std::collections::BTreeMap::new();
    for (from, to) in arguments
        .get("map")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        let to = to
            .as_str()
            .ok_or_else(|| ToolError::invalid(format!("map.{from} must be a field name")))?;
        map.insert(from.clone(), to.to_string());
    }
    let max_rows = match arguments.get("max_rows") {
        None => 100_000,
        Some(value) => value
            .as_u64()
            .filter(|value| *value > 0)
            .ok_or_else(|| ToolError::invalid("`max_rows` must be a positive whole number"))?,
    };
    let request = commands::datatable_copy::DataTableCopyRequest {
        old: name("old")?,
        new: name("new")?,
        map,
        drop_unmapped: flag(arguments, "drop_unmapped", false),
        append: flag(arguments, "append", false),
        max_rows,
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        profile: text(arguments, "profile").unwrap_or("default").to_string(),
    };
    let mut notices = commands::Notices::default();
    let outcome =
        commands::datatable_copy::execute(solution, &request, server::Client::new, &mut notices)
            .map_err(ToolError::coded)?;
    let report = match outcome {
        commands::datatable_copy::DataTableCopyOutcome::Plan { report, .. }
        | commands::datatable_copy::DataTableCopyOutcome::Applied { report, .. } => report,
    };
    let mut result = json!({ "ok": true, "report": report });
    result[if dry_run { "plan" } else { "applied" }] = json!(true);
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn db_clean_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let dry_run = flag(arguments, "dry_run", true);
    let request = commands::db::DbRequest::Clean {
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        profile: text(arguments, "profile").unwrap_or("default").to_string(),
    };
    let mut notices = commands::Notices::default();
    let swept = match commands::db::execute(solution, &request, server::Client::new, &mut notices)
        .map_err(ToolError::coded)?
    {
        commands::db::DbOutcome::Cleaned { things, .. } => things,
        commands::db::DbOutcome::Executed { .. } => unreachable!(),
    };
    let failed = swept
        .iter()
        .any(|thing| thing.status == db::SweepStatus::Failed);
    let mut result = json!({ "ok": !failed, "things": swept });
    result[if dry_run { "plan" } else { "applied" }] = json!(true);
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn db_tool(
    solution: &Solution,
    arguments: &Value,
    mode: db::Mode,
) -> Result<Value, ToolError> {
    let sql = match (text(arguments, "file"), text(arguments, "sql")) {
        (Some(file), None) if !file.is_empty() => {
            let candidate = solution.root.join(file);
            let real = std::fs::canonicalize(&candidate)
                .map_err(|error| ToolError::with(ErrorCode::IoError, format!("{file}: {error}")))?;
            let root = std::fs::canonicalize(&solution.root).map_err(ToolError::io)?;
            if !real.starts_with(&root) {
                return Err(ToolError::invalid(format!(
                    "{file} is outside the solution"
                )));
            }
            std::fs::read_to_string(&real)
                .map_err(|error| ToolError::with(ErrorCode::IoError, format!("{file}: {error}")))?
        }
        (None, Some(sql)) if !sql.is_empty() => sql.to_string(),
        _ => return Err(ToolError::invalid("give exactly one of `file` or `sql`")),
    };
    let positive = |name: &str, default: u64| -> Result<u64, ToolError> {
        match arguments.get(name) {
            None => Ok(default),
            Some(value) => value.as_u64().filter(|value| *value > 0).ok_or_else(|| {
                ToolError::invalid(format!("`{name}` must be a positive whole number"))
            }),
        }
    };
    let timeout = positive("timeout", 120)?;
    let max_rows = positive("max_rows", 500)?;
    let options = db::Options {
        mode,
        thing: text(arguments, "thing").map(str::to_string),
        apply: mode == db::Mode::Query || !flag(arguments, "dry_run", true),
        no_transaction: flag(arguments, "no_transaction", false),
        max_rows,
        timeout: Duration::from_secs(timeout),
    };
    let request = commands::db::DbRequest::Execute {
        sql,
        options,
        profile: text(arguments, "profile").unwrap_or("default").to_string(),
    };
    let mut notices = commands::Notices::default();
    let report = match commands::db::execute(solution, &request, server::Client::new, &mut notices)
        .map_err(ToolError::coded)?
    {
        commands::db::DbOutcome::Executed { report, .. } => report,
        commands::db::DbOutcome::Cleaned { .. } => unreachable!(),
    };
    let mut value = serde_json::to_value(report).expect("db report serialises");
    if mode == db::Mode::Query {
        if let Some(result) = value.get_mut("result").and_then(Value::as_object_mut) {
            let total = result
                .get("rows")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0);
            let columns: Vec<String> = result
                .get("dataShape")
                .and_then(|shape| shape.get("fieldDefinitions"))
                .and_then(Value::as_object)
                .map(|fields| fields.keys().cloned().collect())
                .or_else(|| {
                    result
                        .get("rows")
                        .and_then(Value::as_array)
                        .and_then(|rows| rows.first())
                        .and_then(Value::as_object)
                        .map(|row| row.keys().cloned().collect())
                })
                .unwrap_or_default();
            if !flag(arguments, "detail", false) {
                if let Some(rows) = result.get_mut("rows").and_then(Value::as_array_mut) {
                    rows.truncate(20);
                }
            }
            result.insert("total_rows".to_string(), json!(total));
            result.insert("columns".to_string(), json!(columns));
        }
    }
    add_notices(&mut value, &notices);
    Ok(value)
}

pub(crate) fn db_run_tool(
    solution: &Solution,
    request: crate::mcp::requests::data::DbRunRequest,
) -> Result<Value, ToolError> {
    let sql = match (request.file.as_ref(), request.sql.as_ref()) {
        (Some(file), None) if !file.is_empty() => {
            let candidate = solution.root.join(file);
            let real = std::fs::canonicalize(&candidate)
                .map_err(|error| ToolError::with(ErrorCode::IoError, format!("{file}: {error}")))?;
            let root = std::fs::canonicalize(&solution.root).map_err(ToolError::io)?;
            if !real.starts_with(&root) {
                return Err(ToolError::invalid(format!(
                    "{file} is outside the solution"
                )));
            }
            std::fs::read_to_string(&real)
                .map_err(|error| ToolError::with(ErrorCode::IoError, format!("{file}: {error}")))?
        }
        (None, Some(sql)) if !sql.is_empty() => sql.clone(),
        _ => return Err(ToolError::invalid("give exactly one of `file` or `sql`")),
    };
    let options = db::Options {
        mode: db::Mode::Run,
        thing: request.thing.as_ref().cloned(),
        apply: !request.dry_run,
        no_transaction: request.no_transaction,
        max_rows: 500,
        timeout: Duration::from_secs(request.timeout),
    };
    let request = commands::db::DbRequest::Execute {
        sql,
        options,
        profile: request.profile,
    };
    let mut notices = commands::Notices::default();
    let report = match commands::db::execute(solution, &request, server::Client::new, &mut notices)
        .map_err(ToolError::coded)?
    {
        commands::db::DbOutcome::Executed { report, .. } => report,
        commands::db::DbOutcome::Cleaned { .. } => unreachable!(),
    };
    let mut value = serde_json::to_value(report).expect("db report serialises");
    add_notices(&mut value, &notices);
    Ok(value)
}

pub(crate) fn config_table_tool(
    solution: &Solution,
    arguments: &Value,
) -> Result<Value, ToolError> {
    let thing_arg = required(arguments, "thing")?;
    let table = required(arguments, "table")?;
    let action = text(arguments, "action").unwrap_or("read");
    // As on the command line: a Thing of the solution may be named by its last segment, one not
    // in the solution is taken as given (diff aside), and anything else is refused.
    let found = workspace::discover(solution).entities;
    let resolved = match workspace::resolve(&found, thing_arg) {
        Ok(entity) if entity.info.collection == "Things" => Some(entity),
        Ok(entity) => {
            return Err(ToolError::invalid(format!(
                "{} is a {}, and only a Thing has configuration tables here",
                entity.info.name, entity.info.collection
            )))
        }
        Err(workspace::WorkspaceError::UnknownEntity { .. }) => None,
        Err(error) => return Err(ToolError::coded(error)),
    };
    let thing = resolved
        .as_ref()
        .map(|e| e.info.name.clone())
        .unwrap_or_else(|| thing_arg.to_string());
    if !matches!(action, "read" | "diff" | "restore") {
        return Err(ToolError::invalid(format!(
            "action must be read, diff or restore, not {action:?}"
        )));
    }
    let request = if action == "restore" {
        let backup = required(arguments, "backup")?;
        let backup = {
            let path = PathBuf::from(backup);
            if path.is_absolute() {
                path
            } else {
                solution.root.join(path)
            }
        };
        let dry_run = flag(arguments, "dry_run", true);
        commands::config_table::ConfigTableRequest {
            thing: thing.clone(),
            table: table.to_string(),
            action: commands::config_table::ConfigTableAction::Restore {
                path: backup,
                mode: if dry_run { Mode::Plan } else { Mode::Apply },
            },
            profile: text(arguments, "profile").unwrap_or("default").to_string(),
        }
    } else if action == "diff" {
        let entity = resolved.as_ref().ok_or_else(|| {
            ToolError::with(
                ErrorCode::UnknownEntity,
                format!("{thing_arg} is not an entity of this solution"),
            )
        })?;
        commands::config_table::ConfigTableRequest {
            thing: thing.clone(),
            table: table.to_string(),
            action: commands::config_table::ConfigTableAction::Diff {
                entity: entity.path.clone(),
            },
            profile: text(arguments, "profile").unwrap_or("default").to_string(),
        }
    } else {
        commands::config_table::ConfigTableRequest {
            thing: thing.clone(),
            table: table.to_string(),
            action: commands::config_table::ConfigTableAction::Read,
            profile: text(arguments, "profile").unwrap_or("default").to_string(),
        }
    };
    let mut notices = commands::Notices::default();
    let outcome =
        commands::config_table::execute(solution, &request, server::Client::new, &mut notices)
            .map_err(ToolError::coded)?;
    let mut result = match outcome {
        commands::config_table::ConfigTableOutcome::Restored { plan, .. } => {
            let dry_run = flag(arguments, "dry_run", true);
            json!({
                "thing": thing,
                "table": table,
                "dry_run": dry_run,
                "rows_written": plan.writes,
                "rows_removed": plan.deletes,
                "note": if dry_run { "nothing was written; pass dry_run: false to restore" } else { "restored and read back" },
            })
        }
        commands::config_table::ConfigTableOutcome::Read { table: live, .. } => {
            let key = config_table::primary_key(&live.data_shape);
            let detail = flag(arguments, "detail", false);
            let mut result = json!({
                "thing": thing,
                "table": table,
                "rows": live.rows.len(),
                "primary_key": key,
            });
            result[if detail { "values" } else { "first_rows" }] = json!(live
                .rows
                .iter()
                .take(if detail { usize::MAX } else { 3 })
                .collect::<Vec<_>>());
            result
        }
        commands::config_table::ConfigTableOutcome::Diffed {
            table: live,
            differences,
            ..
        } => json!({
            "thing": thing,
            "table": table,
            "identical": differences.is_empty(),
            "rows": live.rows.len(),
            "differences": differences,
        }),
        commands::config_table::ConfigTableOutcome::BackedUp { .. } => unreachable!(),
    };
    add_notices(&mut result, &notices);
    Ok(result)
}

/// The server's file repositories, read-only.
/// One of the server's logs, summary first. Read-only, so it takes no lock.
pub(crate) fn logs_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let log = required(arguments, "log")?;
    let now = logs::now_ms();
    let (from_ms, to_ms) = match (
        text(arguments, "since"),
        text(arguments, "from"),
        text(arguments, "to"),
    ) {
        (Some(_), Some(_), _) | (Some(_), _, Some(_)) => {
            return Err(ToolError::invalid(
                "since is a window ending now; give it, or from and to, not both",
            ))
        }
        (since, None, None) => (
            now - logs::parse_since(since.unwrap_or("1h")).map_err(ToolError::coded)?,
            now,
        ),
        (None, from, to) => {
            let to_ms = logs::parse_time(to.unwrap_or("now"), now).map_err(ToolError::coded)?;
            let from_ms = match from {
                Some(from) => logs::parse_time(from, now).map_err(ToolError::coded)?,
                None => to_ms - 3_600_000,
            };
            (from_ms, to_ms)
        }
    };
    let search = match (text(arguments, "grep"), text(arguments, "regex")) {
        (Some(_), Some(_)) => {
            return Err(ToolError::invalid(
                "grep and regex are two ways to search; give one",
            ))
        }
        (Some(grep), None) => Some(logs::Search::Grep(grep.to_string())),
        (None, Some(regex)) => Some(logs::Search::Regex(regex.to_string())),
        (None, None) => None,
    };
    let query = logs::Query {
        log: log.to_string(),
        from_ms,
        to_ms,
        level: text(arguments, "level")
            .map(logs::level)
            .transpose()
            .map_err(ToolError::coded)?,
        search,
        user: text(arguments, "user").map(str::to_string),
        thread: text(arguments, "thread").map(str::to_string),
        origin: text(arguments, "origin").map(str::to_string),
        limit: arguments
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(200),
        oldest_first: flag(arguments, "oldest_first", false),
    };
    let client = client(solution, arguments)?;
    let outcome = logs::query(&client, &query).map_err(ToolError::coded)?;
    Ok(logs::summary(
        log,
        &outcome,
        flag(arguments, "detail", false),
    ))
}

/// A log's levels, read, or changed as a plan unless dry_run is false. Writes no workspace file.
pub(crate) fn log_level_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let log = required(arguments, "log")?;
    let sublogger = text(arguments, "sublogger").map(str::to_string);
    let reset = flag(arguments, "reset", false);
    let change = match (text(arguments, "level"), reset) {
        (Some(_), true) => return Err(ToolError::invalid("give a level or reset, not both")),
        (Some(level), false) => Some(logs::Change::Set {
            level: logs::level(level).map_err(ToolError::coded)?,
            sublogger,
        }),
        (None, true) => Some(logs::Change::Reset { sublogger }),
        (None, false) if sublogger.is_some() => {
            return Err(ToolError::invalid(
                "a sublogger needs a level to set, or reset",
            ))
        }
        (None, false) => None,
    };
    let levels_json = |levels: &logs::Levels| {
        json!({
            "level": levels.level,
            "subloggers": levels.subloggers.iter().map(|(name, level)| json!({ "sublogger": name, "level": level })).collect::<Vec<_>>(),
        })
    };
    let dry_run = flag(arguments, "dry_run", true);
    let request = commands::logs::LogLevelRequest {
        log: log.to_string(),
        change,
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        profile: text(arguments, "profile").unwrap_or("default").to_string(),
    };
    let mut notices = commands::Notices::default();
    let outcome = commands::logs::execute(solution, &request, server::Client::new, &mut notices)
        .map_err(ToolError::coded)?;
    let commands::logs::LogLevelOutcome::Levels { levels, .. } = outcome else {
        let report = match outcome {
            commands::logs::LogLevelOutcome::Plan { report, .. }
            | commands::logs::LogLevelOutcome::Applied { report, .. } => report,
            commands::logs::LogLevelOutcome::Levels { .. } => unreachable!(),
        };
        let mut result = json!({
            "ok": true,
            "log": log,
            "dry_run": dry_run,
            "change": report.plan,
            "before": levels_json(&report.before),
            "undo": report.undo,
        });
        match &report.after {
            Some(after) => result["after"] = levels_json(after),
            None => {
                result["note"] =
                    json!("nothing was sent; pass dry_run: false. The level is the whole server's")
            }
        }
        add_notices(&mut result, &notices);
        return Ok(result);
    };
    let mut result = json!({ "ok": true, "log": log, "levels": levels_json(&levels) });
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn call_service_tool(
    solution: &Solution,
    arguments: &Value,
) -> Result<Value, ToolError> {
    let target_arg = required(arguments, "target")?;
    let service = required(arguments, "service")?;
    let parameters = arguments
        .get("parameters")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if !parameters.is_object() {
        return Err(ToolError::invalid("`parameters` must be a JSON object"));
    }
    let timeout = arguments
        .get("timeout_seconds")
        .and_then(Value::as_u64)
        .filter(|s| *s > 0)
        .unwrap_or(120);
    let dry_run = flag(arguments, "dry_run", true);
    let request = commands::call::CallRequest {
        target: target_arg.to_string(),
        service: service.to_string(),
        parameters: parameters.clone(),
        timeout: Duration::from_secs(timeout),
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        profile: text(arguments, "profile").unwrap_or("default").to_string(),
        with_logs: flag(arguments, "with_logs", false),
        profile_before_target: false,
    };
    let mut notices = commands::Notices::default();
    let outcome = commands::call::execute(solution, &request, server::Client::new, &mut notices)
        .map_err(ToolError::coded)?;
    let commands::call::CallOutcome::Applied {
        reply,
        logs: logged,
        ..
    } = outcome
    else {
        let target = outcome.target();
        return Ok(json!({
            "dry_run": true,
            "would_call": { "target": target.to_string(), "service": service, "parameters": parameters },
            "note": "nothing was sent; pass dry_run: false to call it",
        }));
    };
    let logs_json = |found: &Result<Vec<(String, logs::Entry)>, logs::LogsError>| match found {
        Ok(entries) => json!(entries
            .iter()
            .map(|(log, entry)| {
                let mut value = logs::entry_json(entry);
                value["log"] = json!(log);
                value
            })
            .collect::<Vec<_>>()),
        Err(error) => json!({ "error": format!("the logs could not be read: {error}") }),
    };
    let detail = flag(arguments, "detail", false);
    let mut result = match reply {
        None => json!({ "dry_run": false, "result": "void" }),
        Some(value) if !detail && value.get("rows").is_some_and(Value::is_array) => {
            let rows = value["rows"].as_array().expect("checked");
            let fields: Vec<&String> = value
                .pointer("/dataShape/fieldDefinitions")
                .and_then(Value::as_object)
                .map(|f| f.keys().collect())
                .unwrap_or_default();
            json!({
                "dry_run": false,
                "rows": rows.len(),
                "fields": fields,
                "first_rows": rows.iter().take(3).collect::<Vec<_>>(),
            })
        }
        Some(value) => {
            // Not an InfoTable, so it has no rows to summarise; bound it by size instead.
            const SUMMARY_BYTES: usize = 16 * 1024;
            let text = value.to_string();
            if detail || text.len() <= SUMMARY_BYTES {
                json!({ "dry_run": false, "result": value })
            } else {
                let head: String = text.chars().take(4096).collect();
                json!({
                    "dry_run": false,
                    "result_bytes": text.len(),
                    "result_head": head,
                    "note": "the result is long; pass detail: true for all of it",
                })
            }
        }
    };
    if let Some(found) = &logged {
        result["logs"] = logs_json(found);
    }
    add_notices(&mut result, &notices);
    Ok(result)
}
