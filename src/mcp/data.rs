use super::requests::data::{
    CallRequest, ConfigTableAction as TableAction, ConfigTableRequest, DatatableCopyRequest,
    DbCleanRequest, DbQueryRequest, DbRunRequest, LogLevelRequest, LogsRequest,
};
use super::*;

pub(crate) fn datatable_copy_tool(
    solution: &Solution,
    arguments: DatatableCopyRequest,
) -> Result<Value, ToolError> {
    let dry_run = arguments.dry_run;
    let request = commands::datatable_copy::DataTableCopyRequest {
        old: nonempty(&arguments.old, "old")?.to_string(),
        new: nonempty(&arguments.new, "new")?.to_string(),
        map: arguments.map.as_ref().cloned().unwrap_or_default(),
        drop_unmapped: arguments.drop_unmapped,
        append: arguments.append,
        max_rows: arguments.max_rows,
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        profile: arguments.profile,
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

pub(crate) fn db_clean_tool(
    solution: &Solution,
    arguments: DbCleanRequest,
) -> Result<Value, ToolError> {
    let dry_run = arguments.dry_run;
    let request = commands::db::DbRequest::Clean {
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        profile: arguments.profile,
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

/// The SQL a tool was given, from a file of the solution or inline: exactly one of the two.
fn sql_source(
    solution: &Solution,
    file: &requests::common::Absent<String>,
    sql: &requests::common::Absent<String>,
) -> Result<String, ToolError> {
    match (file.as_ref(), sql.as_ref()) {
        (Some(file), None) if !file.is_empty() => {
            let real = in_path(solution, file)?;
            std::fs::read_to_string(&real)
                .map_err(|error| ToolError::with(ErrorCode::IoError, format!("{file}: {error}")))
        }
        (None, Some(sql)) if !sql.is_empty() => Ok(sql.clone()),
        _ => Err(ToolError::invalid("give exactly one of `file` or `sql`")),
    }
}

pub(crate) fn db_query_tool(
    solution: &Solution,
    arguments: DbQueryRequest,
) -> Result<Value, ToolError> {
    let sql = sql_source(solution, &arguments.file, &arguments.sql)?;
    let options = db::Options {
        mode: db::Mode::Query,
        thing: arguments.thing.as_ref().cloned(),
        apply: true,
        no_transaction: false,
        max_rows: arguments.max_rows,
        timeout: Duration::from_secs(arguments.timeout),
    };
    let request = commands::db::DbRequest::Execute {
        sql,
        options,
        profile: arguments.profile,
    };
    let mut notices = commands::Notices::default();
    let report = match commands::db::execute(solution, &request, server::Client::new, &mut notices)
        .map_err(ToolError::coded)?
    {
        commands::db::DbOutcome::Executed { report, .. } => report,
        commands::db::DbOutcome::Cleaned { .. } => unreachable!(),
    };
    let mut value = serde_json::to_value(report).expect("db report serialises");
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
        if !arguments.detail {
            if let Some(rows) = result.get_mut("rows").and_then(Value::as_array_mut) {
                rows.truncate(20);
            }
        }
        result.insert("total_rows".to_string(), json!(total));
        result.insert("columns".to_string(), json!(columns));
    }
    add_notices(&mut value, &notices);
    Ok(value)
}

pub(crate) fn db_run_tool(solution: &Solution, request: DbRunRequest) -> Result<Value, ToolError> {
    let sql = sql_source(solution, &request.file, &request.sql)?;
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
    arguments: ConfigTableRequest,
) -> Result<Value, ToolError> {
    let thing_arg = nonempty(&arguments.thing, "thing")?;
    let table = nonempty(&arguments.table, "table")?;
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
    let request = if arguments.action == TableAction::Restore {
        let backup = in_path(solution, required_text(&arguments.backup, "backup")?)?;
        let dry_run = arguments.dry_run;
        commands::config_table::ConfigTableRequest {
            thing: thing.clone(),
            table: table.to_string(),
            action: commands::config_table::ConfigTableAction::Restore {
                path: backup,
                mode: if dry_run { Mode::Plan } else { Mode::Apply },
            },
            profile: arguments.profile.clone(),
        }
    } else if arguments.action == TableAction::Backup {
        // Never true: core refuses to replace a backup, which is the point of one.
        let path = out_path(solution, required_text(&arguments.backup, "backup")?, false)?;
        commands::config_table::ConfigTableRequest {
            thing: thing.clone(),
            table: table.to_string(),
            action: commands::config_table::ConfigTableAction::Backup { path },
            profile: arguments.profile.clone(),
        }
    } else if arguments.action == TableAction::Diff {
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
            profile: arguments.profile.clone(),
        }
    } else {
        commands::config_table::ConfigTableRequest {
            thing: thing.clone(),
            table: table.to_string(),
            action: commands::config_table::ConfigTableAction::Read,
            profile: arguments.profile.clone(),
        }
    };
    let mut notices = commands::Notices::default();
    let outcome =
        commands::config_table::execute(solution, &request, server::Client::new, &mut notices)
            .map_err(ToolError::coded)?;
    let mut result = match outcome {
        commands::config_table::ConfigTableOutcome::Restored { plan, .. } => {
            let dry_run = arguments.dry_run;
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
            let detail = arguments.detail;
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
        commands::config_table::ConfigTableOutcome::BackedUp { table: live, .. } => json!({
            "thing": thing,
            "table": table,
            "rows": live.rows.len(),
            "backup": required_text(&arguments.backup, "backup")?,
            "next": "restore it with action restore and this backup",
        }),
    };
    add_notices(&mut result, &notices);
    Ok(result)
}

/// The server's file repositories, read-only.
/// One of the server's logs, summary first. Read-only, so it takes no lock.
pub(crate) fn logs_tool(solution: &Solution, arguments: LogsRequest) -> Result<Value, ToolError> {
    let log = nonempty(&arguments.log, "log")?;
    let now = logs::now_ms();
    let (from_ms, to_ms) = match (
        arguments.since.as_deref(),
        arguments.from.as_deref(),
        arguments.to.as_deref(),
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
    let search = match (arguments.grep.as_deref(), arguments.regex.as_deref()) {
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
        level: arguments
            .level
            .map(|level| level.as_str())
            .map(logs::level)
            .transpose()
            .map_err(ToolError::coded)?,
        search,
        user: arguments.user.as_ref().cloned(),
        thread: arguments.thread.as_ref().cloned(),
        origin: arguments.origin.as_ref().cloned(),
        limit: arguments.limit,
        oldest_first: arguments.oldest_first,
    };
    let client = client(solution, &arguments.profile)?;
    let outcome = logs::query(&client, &query).map_err(ToolError::coded)?;
    Ok(logs::summary(log, &outcome, arguments.detail))
}

/// A log's levels, read, or changed as a plan unless dry_run is false. Writes no workspace file.
pub(crate) fn log_level_tool(
    solution: &Solution,
    arguments: LogLevelRequest,
) -> Result<Value, ToolError> {
    let log = nonempty(&arguments.log, "log")?;
    let sublogger = arguments.sublogger.as_ref().cloned();
    let reset = arguments.reset;
    let change = match (arguments.level.map(|level| level.as_str()), reset) {
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
    let dry_run = arguments.dry_run;
    let request = commands::logs::LogLevelRequest {
        log: log.to_string(),
        change,
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        profile: arguments.profile.clone(),
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
    arguments: CallRequest,
) -> Result<Value, ToolError> {
    let target_arg = nonempty(&arguments.target, "target")?;
    let service = nonempty(&arguments.service, "service")?;
    let parameters = Value::Object(arguments.parameters.clone());
    let timeout = arguments.timeout_seconds;
    let dry_run = arguments.dry_run;
    let request = commands::call::CallRequest {
        target: target_arg.to_string(),
        service: service.to_string(),
        parameters: parameters.clone(),
        timeout: Duration::from_secs(timeout),
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        profile: arguments.profile.clone(),
        with_logs: arguments.with_logs,
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
    let detail = arguments.detail;
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
