use super::*;

pub(crate) fn status_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let record = flag(arguments, "record", false);
    let target = match (text(arguments, "entity"), flag(arguments, "all", false)) {
        (Some(_), true) => {
            return Err(ToolError::invalid(
                "name an entity or pass all: true, not both",
            ))
        }
        (Some(name), false) => commands::status::StatusTarget::Names(vec![name.to_string()]),
        (None, true) => commands::status::StatusTarget::All,
        (None, false) => return Err(ToolError::invalid("name an entity, or pass all: true")),
    };
    let request = commands::status::StatusRequest {
        target,
        project: text(arguments, "project").map(str::to_string),
        record,
        profile: text(arguments, "profile").unwrap_or("default").to_string(),
        lock_label: "mcp status --record",
        refuse_unreadable: record,
        refuse_record_failures: record,
    };
    let mut notices = commands::Notices::default();
    let outcome =
        match commands::status::execute(solution, &request, server::Client::new, &mut notices) {
            Ok(outcome) => outcome,
            Err(commands::status::StatusCommandError::Unreadable(items)) if record => {
                return Err(ToolError::with(
                    ErrorCode::InvalidData,
                    format!(
                        "nothing was recorded: {} entity file(s) could not be read: {}",
                        items.len(),
                        items.join("; ")
                    ),
                ));
            }
            Err(error) => return Err(ToolError::coded(error)),
        };
    let counts: Map<String, Value> = status::counts(&outcome.statuses)
        .into_iter()
        .map(|(v, n)| (v.label().to_string(), json!(n)))
        .collect();
    let detail = flag(arguments, "detail", false);
    let listed: Vec<Value> = outcome.statuses
        .iter()
        .filter(|s| detail || s.verdict.is_drift())
        .map(|s| {
            let mut entry = json!({ "entity": format!("{}/{}", s.collection, s.name), "verdict": s.verdict.label() });
            if detail {
                entry["working"] = json!(s.working);
                entry["server"] = json!(s.server);
                entry["baseline_local"] = json!(s.local_baseline);
                entry["baseline_server"] = json!(s.server_baseline);
            }
            entry
        })
        .collect();
    let mut result = json!({
        "ok": outcome.failures.is_empty() && outcome.statuses.iter().all(|s| !s.verdict.is_drift()),
        "recorded": outcome.recorded,
        "entities": outcome.statuses.len(),
        "counts": counts,
        "failures": outcome.failures,
        "unreadable": outcome.unreadable,
    });
    // Summary: only what needs attention. Detail: every entity, with its hashes.
    result[if detail { "statuses" } else { "attention" }] = json!(listed);
    add_notices(&mut result, &notices);
    Ok(result)
}

/// The optional entity spelling that an executor will validate after taking a write lock.
pub(crate) fn tool_target(arguments: &Value) -> Vec<String> {
    text(arguments, "entity")
        .map(str::to_string)
        .into_iter()
        .collect()
}

pub(crate) fn refusal_code(refusal: &push::Refusal) -> &'static str {
    match refusal {
        push::Refusal::Conflict { .. } => "server-changed",
        push::Refusal::UnknownAncestor { .. } => "no-baseline",
        push::Refusal::DeletedOnServer => "deleted-on-server",
    }
}

pub(crate) fn push_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let name = required(arguments, "entity")?;
    let dry_run = flag(arguments, "dry_run", true);
    let force = flag(arguments, "force", false);
    let request = commands::push::PushRequest {
        entity: name.to_string(),
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        force,
        backup: flag(arguments, "backup", true),
        profile: text(arguments, "profile").unwrap_or("default").to_string(),
    };
    let mut notices = commands::Notices::default();
    let outcome = commands::push::execute(solution, &request, server::Client::new, &mut notices)
        .map_err(|error| ToolError {
            code: error.code(),
            message: match error.backup() {
                Some(dir) => {
                    format!("{error}; the server's copy was saved to {dir} before the push")
                }
                None => error.to_string(),
            },
        })?;
    let label = match &outcome {
        commands::push::PushOutcome::Plan { entity, .. }
        | commands::push::PushOutcome::Applied { entity, .. } => entity.to_string(),
    };
    let saved = match &outcome {
        commands::push::PushOutcome::Applied { backup, .. } => backup.clone(),
        commands::push::PushOutcome::Plan { .. } => None,
    };
    let mut result = push_outcome_json(&label, dry_run, force, outcome);
    if let Some(dir) = saved {
        result["backup"] = json!(dir);
    }
    add_notices(&mut result, &notices);
    Ok(result)
}

/// What taking the workspace lock did (files swept, interrupted operations recovered), when it
/// did anything.
pub(crate) fn push_outcome_json(
    label: &str,
    dry_run: bool,
    force: bool,
    outcome: commands::push::PushOutcome,
) -> Value {
    match outcome {
        commands::push::PushOutcome::Plan { decision, .. } => {
            let (would, refusal) = match &decision {
                push::Decision::AlreadyThere => {
                    ("nothing: the server already has this version", None)
                }
                push::Decision::Create => ("create it on the server", None),
                push::Decision::Update => (
                    "update it; the server is unchanged since the last sync",
                    None,
                ),
                push::Decision::Refuse(refusal) => (
                    "refuse",
                    Some((refusal.to_string(), refusal.code().as_str())),
                ),
            };
            match refusal {
                Some((refusal, code)) => {
                    json!({ "entity": label, "dry_run": dry_run, "would": would, "refusal": refusal, "code": code, "force": force })
                }
                None => {
                    json!({ "entity": label, "dry_run": dry_run, "would": would, "refusal": null, "force": force })
                }
            }
        }
        commands::push::PushOutcome::Applied {
            result: push::Outcome::AlreadyThere,
            ..
        } => {
            json!({ "entity": label, "dry_run": dry_run, "pushed": false, "note": "the server already has this version; baseline recorded" })
        }
        commands::push::PushOutcome::Applied {
            result: push::Outcome::Pushed { created },
            ..
        } => {
            json!({ "entity": label, "dry_run": dry_run, "pushed": true, "created": created, "note": "read back and matching; baseline recorded" })
        }
        commands::push::PushOutcome::Applied {
            result: push::Outcome::Refused(refusal),
            ..
        } => {
            json!({ "entity": label, "dry_run": dry_run, "pushed": false, "refusal": refusal.to_string(), "code": refusal.code().as_str(), "note": "nothing was sent; force: true pushes anyway" })
        }
        commands::push::PushOutcome::Applied {
            result: push::Outcome::WouldDo(_),
            ..
        } => unreachable!("an applied outcome cannot be a plan"),
    }
}

pub(crate) fn entity_delete_tool(
    solution: &Solution,
    arguments: &Value,
) -> Result<Value, ToolError> {
    let dry_run = flag(arguments, "dry_run", true);
    let (acknowledged, force_used) = entity_delete::acknowledged(
        flag(arguments, "force", false),
        flag(arguments, "allow_repository_defined", false),
        flag(arguments, "allow_outside_dependents", false),
        flag(arguments, "allow_file_repository_data_loss", false),
    );
    let request = commands::delete::EntityDeleteRequest {
        entities: strings(arguments, "entities"),
        renamed: flag(arguments, "renamed", false),
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        acknowledgements: acknowledged,
        legacy_force_used: force_used,
        backup: flag(arguments, "backup", true),
        profile: text(arguments, "profile").unwrap_or("default").to_string(),
    };
    let mut notices = commands::Notices::default();
    let outcome = commands::delete::execute(solution, &request, server::Client::new, &mut notices)
        .map_err(|error| ToolError {
            code: error.code(),
            message: error.to_string(),
        })?;
    let (report, date, force_used) = match outcome {
        commands::delete::EntityDeleteOutcome::Plan {
            report,
            legacy_force_used,
            ..
        } => (report, None, legacy_force_used),
        commands::delete::EntityDeleteOutcome::Applied {
            report,
            date,
            legacy_force_used,
            ..
        } => (report, Some(date), legacy_force_used),
    };
    let mut result = json!({
        "ok": !report.failed(),
        "entities": report.entities,
        "dependency_limit": report.dependency_limit,
    });
    if let Some(dir) = &report.backup {
        result["backup"] = json!(dir);
    }
    result[if dry_run { "plan" } else { "applied" }] = json!(true);
    if report.ledger_changed {
        result["ledger_marked"] = json!(date.expect("an applied delete outcome has a date"));
    }
    if force_used {
        result["deprecated"] = json!(entity_delete::FORCE_DEPRECATION);
    }
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn entity_restore_tool(
    solution: &Solution,
    arguments: &Value,
) -> Result<Value, ToolError> {
    let dry_run = flag(arguments, "dry_run", true);
    let request = commands::restore::RestoreRequest {
        set: text(arguments, "set")
            .filter(|id| !id.is_empty())
            .map(str::to_string),
        only: strings(arguments, "entities"),
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        profile: text(arguments, "profile").unwrap_or("default").to_string(),
    };
    let mut notices = commands::Notices::default();
    let outcome = commands::restore::execute(solution, &request, server::Client::new, &mut notices)
        .map_err(ToolError::coded)?;
    let mut result = match outcome {
        commands::restore::RestoreOutcome::Sets { sets, .. } => {
            json!({ "ok": true, "sets": sets.iter().map(|set| json!({
                "id": set.id, "created": set.manifest.created, "reason": set.manifest.reason,
                "entities": set.manifest.entities.iter().map(|item| format!("{}/{}", item.collection, item.name)).collect::<Vec<_>>(),
            })).collect::<Vec<_>>() })
        }
        commands::restore::RestoreOutcome::Plan { set, entities, .. }
        | commands::restore::RestoreOutcome::Applied { set, entities, .. } => {
            let failed = entities
                .iter()
                .any(|entry| entry.status == backup::Status::Failed);
            let mut result = json!({ "ok": !failed, "set": set.id, "entities": entities });
            result[if dry_run { "plan" } else { "applied" }] = json!(true);
            result
        }
    };
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn entity_carry_tool(
    solution: &Solution,
    arguments: &Value,
) -> Result<Value, ToolError> {
    let dry_run = flag(arguments, "dry_run", true);
    let pairs =
        entity_carry::pairs_from_names(&strings(arguments, "pairs")).map_err(ToolError::coded)?;
    let request = commands::carry::CarryRequest {
        pairs,
        renamed: flag(arguments, "renamed", false),
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        detail: flag(arguments, "detail", false),
        profile: text(arguments, "profile").unwrap_or("default").to_string(),
        lock_label: "mcp entity_carry",
    };
    let mut notices = commands::Notices::default();
    let outcome = commands::carry::execute(solution, &request, server::Client::new, &mut notices)
        .map_err(ToolError::coded)?;
    let (report, date) = match outcome {
        commands::carry::CarryOutcome::Plan { report, .. } => (report, None),
        commands::carry::CarryOutcome::Applied { report, date, .. } => (report, Some(date)),
    };
    let failed = !dry_run
        && report
            .entities
            .iter()
            .any(|entity| entity.status == entity_carry::Status::Failed);
    let mut result = json!({ "ok": !failed, "entities": report.entities });
    result[if dry_run { "plan" } else { "applied" }] = json!(true);
    if report.ledger_changed {
        result["ledger_marked"] = json!(date.expect("an applied carry has a date"));
    }
    add_notices(&mut result, &notices);
    Ok(result)
}
