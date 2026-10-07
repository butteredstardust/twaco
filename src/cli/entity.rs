use super::super::*;
use super::refactor::same_path;

/// Fetch one raw export without ever replacing a file that belongs to the solution.
pub(crate) fn entity_get(solution: &Solution, args: &Args) -> u8 {
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
        if protected
            .iter()
            .any(|candidate| same_path(out, &candidate.path))
        {
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
    let live = match server::Client::new(profile)
        .fetch_entity(&entity.info.collection, &entity.info.name)
    {
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
pub(crate) fn entity_status(solution: &Solution, args: &Args) -> u8 {
    if args.names.len() > 1 {
        eprintln!("twaco: entity status accepts one entity name, or --all");
        return FAILED;
    }
    let request = commands::status::StatusRequest {
        target: if args.has("--all") {
            commands::status::StatusTarget::All
        } else {
            commands::status::StatusTarget::Names(args.names.clone())
        },
        project: args.project.clone(),
        record: args.has("--record"),
        profile: args
            .profile
            .clone()
            .unwrap_or_else(|| "default".to_string()),
        lock_label: "entity status",
        refuse_unreadable: true,
        refuse_record_failures: false,
    };
    let mut notices = commands::Notices::default();
    let outcome =
        match commands::status::execute(solution, &request, server::Client::new, &mut notices) {
            Ok(outcome) => outcome,
            Err(error) => {
                print_notices(&notices);
                if let commands::status::StatusCommandError::Unreadable(items) = &error {
                    for problem in items {
                        eprintln!("twaco: {problem}");
                    }
                } else {
                    eprintln!("twaco: {error}");
                }
                return FAILED;
            }
        };
    print_notices(&notices);
    if !outcome.failures.is_empty() {
        for failure in &outcome.failures {
            eprintln!("twaco: {failure}");
        }
        eprintln!(
            "twaco: {} entity status request(s) failed",
            outcome.failures.len()
        );
        return FAILED;
    }

    println!("{} entity status(es)", outcome.statuses.len());
    for (verdict, count) in status::counts(&outcome.statuses) {
        println!("  {:<19} {count}", verdict.label());
    }
    if args.has("--detail") {
        println!();
        for status in &outcome.statuses {
            println!(
                "{}/{}  {}",
                status.collection,
                status.name,
                status.verdict.label()
            );
            println!("  working  {}", status.working);
            println!("  server   {}", status.server.as_deref().unwrap_or("-"));
            println!(
                "  baseline local  {}",
                status.local_baseline.as_deref().unwrap_or("-")
            );
            println!(
                "  baseline server {}",
                status.server_baseline.as_deref().unwrap_or("-")
            );
        }
    }
    if outcome
        .statuses
        .iter()
        .all(|status| !status.verdict.is_drift())
    {
        OK
    } else {
        DRIFT
    }
}

/// Push one entity. A dry run unless `--apply`: the blast radius is a server.
pub(crate) fn entity_push(solution: &Solution, args: &Args) -> u8 {
    if args.names.len() != 1 || args.has("--all") {
        eprintln!("twaco: entity push takes exactly one entity name");
        return FAILED;
    }
    let request = commands::push::PushRequest {
        entity: args.names[0].clone(),
        mode: if args.has("--apply") {
            Mode::Apply
        } else {
            Mode::Plan
        },
        force: args.has("--force"),
        backup: !args.has("--no-backup"),
        profile: args
            .profile
            .clone()
            .unwrap_or_else(|| "default".to_string()),
    };
    let mut notices = commands::Notices::default();
    let result = commands::push::execute(solution, &request, server::Client::new, &mut notices);
    print_notices(&notices);
    match result {
        Ok(commands::push::PushOutcome::Plan {
            entity, decision, ..
        }) => {
            let label = entity.to_string();
            match decision {
                push::Decision::AlreadyThere => {
                    println!("{label}: nothing to push")
                }
                push::Decision::Create => println!("{label}: would create it on the server"),
                push::Decision::Update => {
                    println!(
                        "{label}: would update it; the server is unchanged since the last sync"
                    )
                }
                push::Decision::Refuse(refusal) => {
                    println!("{label}: would refuse: {refusal}");
                    if !request.force {
                        return DRIFT;
                    }
                    println!("  --force would push anyway");
                }
            }
            println!("dry run: nothing was sent; pass --apply to push");
            OK
        }
        Ok(commands::push::PushOutcome::Applied {
            entity,
            result,
            backup: saved,
            ..
        }) => {
            let label = entity.to_string();
            if let Some(dir) = saved {
                println!("{label}: the server's copy was saved to {dir} before it is overwritten");
            }
            match result {
                push::Outcome::AlreadyThere => {
                    println!("{label}: nothing to push; baseline is current");
                    OK
                }
                push::Outcome::Pushed { created } => {
                    let verb = if created { "created" } else { "updated" };
                    println!("{label}: {verb}, read back and matching; baseline recorded");
                    OK
                }
                push::Outcome::Refused(refusal) => {
                    eprintln!("twaco: {label}: refused: {refusal}");
                    eprintln!("twaco: nothing was sent; --force pushes anyway");
                    DRIFT
                }
                push::Outcome::WouldDo(_) => unreachable!("an applied outcome cannot be a plan"),
            }
        }
        Err(error) => {
            if let commands::push::PushCommandError::Unreadable(unreadable) = &error {
                for problem in unreadable {
                    eprintln!("twaco: {problem}");
                }
            } else if matches!(error, commands::push::PushCommandError::Backup { .. }) {
                eprintln!("twaco: {error} (--no-backup pushes without one)");
            } else {
                if let (Some(label), Some(dir)) = (error.label(), error.backup()) {
                    println!(
                        "{label}: the server's copy was saved to {dir} before it is overwritten"
                    );
                }
                eprintln!("twaco: {error}");
            }
            FAILED
        }
    }
}

/// Delete server entities in dependency-safe order. Planning is read-only and always succeeds
/// even when it reports guarded refusals; an apply reports any refusal or failed confirmation as
/// exit 2 after attempting the rest.
pub(crate) fn entity_delete_cmd(solution: &Solution, args: &Args) -> u8 {
    let (acknowledged, force_used) = entity_delete::acknowledged(
        args.has("--force"),
        args.has("--allow-repository-defined"),
        args.has("--allow-outside-dependents"),
        args.has("--allow-file-repository-data-loss"),
    );
    if force_used {
        eprintln!("twaco: {}", entity_delete_force_deprecation());
    }
    let request = commands::delete::EntityDeleteRequest {
        entities: args.names.clone(),
        renamed: args.has("--renamed"),
        mode: if args.has("--apply") {
            Mode::Apply
        } else {
            Mode::Plan
        },
        acknowledgements: acknowledged,
        legacy_force_used: force_used,
        backup: !args.has("--no-backup"),
        profile: args
            .profile
            .clone()
            .unwrap_or_else(|| "default".to_string()),
    };
    let mut notices = commands::Notices::default();
    let result = commands::delete::execute(solution, &request, server::Client::new, &mut notices);
    print_notices(&notices);
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let (report, date, apply) = match outcome {
        commands::delete::EntityDeleteOutcome::Plan { report, .. } => (report, None, false),
        commands::delete::EntityDeleteOutcome::Applied { report, date, .. } => {
            (report, Some(date), true)
        }
    };
    if args.has("--json") {
        let key = if apply { "applied" } else { "plan" };
        let value = serde_json::json!({ (key): true, "entities": report.entities, "backup": report.backup });
        println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("delete report serialises")
        );
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
                println!(
                    "     dependent: {}/{}",
                    dependent.collection, dependent.name
                );
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
            println!(
                "rename ledger marked with {}",
                date.expect("an applied delete outcome has a date")
            );
        }
    }
    if apply && report.failed() {
        FAILED
    } else {
        OK
    }
}

pub(crate) fn entity_delete_force_deprecation() -> &'static str {
    entity_delete::FORCE_DEPRECATION
}

/// List backup sets, or plan (and with --apply perform) importing one back.
pub(crate) fn entity_restore_cmd(solution: &Solution, args: &Args) -> u8 {
    let json = args.has("--json");
    let request = commands::restore::RestoreRequest {
        set: args.names.first().cloned(),
        only: args.names.iter().skip(1).cloned().collect(),
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
    let outcome =
        match commands::restore::execute(solution, &request, server::Client::new, &mut notices) {
            Ok(outcome) => outcome,
            Err(error) => {
                print_notices(&notices);
                eprintln!("twaco: {error}");
                return FAILED;
            }
        };
    print_notices(&notices);
    let (set, report, apply) = match outcome {
        commands::restore::RestoreOutcome::Sets { sets, .. } => {
            if json {
                let value = serde_json::json!({ "sets": sets.iter().map(|set| serde_json::json!({
                "id": set.id, "created": set.manifest.created, "reason": set.manifest.reason, "entities": set.manifest.entities.len(),
            })).collect::<Vec<_>>() });
                println!(
                    "{}",
                    serde_json::to_string_pretty(&value).expect("sets serialise")
                );
            } else if sets.is_empty() {
                println!("no backup sets under {}", backup::DIR);
            } else {
                for set in &sets {
                    println!(
                        "{}  {}  {} entit{}",
                        set.id,
                        set.manifest.reason,
                        set.manifest.entities.len(),
                        if set.manifest.entities.len() == 1 {
                            "y"
                        } else {
                            "ies"
                        }
                    );
                }
            }
            return OK;
        }
        commands::restore::RestoreOutcome::Plan { set, entities, .. } => (set, entities, false),
        commands::restore::RestoreOutcome::Applied { set, entities, .. } => (set, entities, true),
    };
    if json {
        let value = serde_json::json!({ (if apply { "applied" } else { "plan" }): true, "set": set.id, "entities": report });
        println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("restore report serialises")
        );
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
    if report
        .iter()
        .any(|entry| entry.status == backup::Status::Failed)
    {
        FAILED
    } else {
        OK
    }
}

/// Copy permissions from old entities to the ones that replaced them. A plan only reads; an apply
/// writes what differs and reports any failure as exit 2 after attempting the rest. Only an apply
/// that reads the ledger's pending entries writes the workspace, and so takes its lock.
pub(crate) fn entity_carry_cmd(solution: &Solution, args: &Args) -> u8 {
    let pairs = match entity_carry::pairs_from_names(&args.names) {
        Ok(pairs) => pairs,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let request = commands::carry::CarryRequest {
        pairs,
        renamed: args.has("--renamed"),
        mode: if args.has("--apply") {
            Mode::Apply
        } else {
            Mode::Plan
        },
        detail: args.has("--detail"),
        profile: args
            .profile
            .clone()
            .unwrap_or_else(|| "default".to_string()),
        lock_label: "entity carry",
    };
    let mut notices = commands::Notices::default();
    let outcome =
        match commands::carry::execute(solution, &request, server::Client::new, &mut notices) {
            Ok(outcome) => outcome,
            Err(error) => {
                print_notices(&notices);
                eprintln!("twaco: {error}");
                return FAILED;
            }
        };
    print_notices(&notices);
    let (report, apply, date) = match outcome {
        commands::carry::CarryOutcome::Plan { report, .. } => (report, false, None),
        commands::carry::CarryOutcome::Applied { report, date, .. } => (report, true, Some(date)),
    };
    if args.has("--json") {
        let key = if apply { "applied" } else { "plan" };
        let value = serde_json::json!({ (key): true, "entities": report.entities });
        println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("carry report serialises")
        );
    } else {
        println!("{}:", if apply { "applied" } else { "plan" });
        for entity in &report.entities {
            let kinds = if entity.kinds.is_empty() {
                String::new()
            } else {
                format!("  [{}]", entity.kinds.join(", "))
            };
            println!(
                "  {}/{} -> {}  {:?}{kinds}",
                entity.collection, entity.old, entity.new, entity.status
            );
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
            println!(
                "rename ledger marked with {}",
                date.expect("an applied carry has a date")
            );
        }
    }
    if report
        .entities
        .iter()
        .any(|entity| entity.status == entity_carry::Status::Failed)
        && apply
    {
        FAILED
    } else {
        OK
    }
}

/// Compare entity permissions with the server's (`permissions diff`), or make them the
/// repository's (`permissions push`). A diff exits 1 when any entity differs, like a check; a push
/// is a plan without `--apply`, and an applied push that fails anywhere exits 2.
pub(crate) fn permissions_cmd(solution: &Solution, route: &str, args: &Args) -> u8 {
    let push = route == "permissions push";
    if push && args.has("--platform") {
        if args.has("--all") || !args.names.is_empty() {
            eprintln!(
                "twaco: --platform pushes the policy's platform entries; push entities separately"
            );
            return FAILED;
        }
        return permissions_platform_cmd(solution, args);
    }
    let request = commands::permissions::PermissionsRequest {
        target: if args.has("--all") {
            commands::status::StatusTarget::All
        } else {
            commands::status::StatusTarget::Names(args.names.clone())
        },
        project: args.project.clone(),
        mode: if push && args.has("--apply") {
            Mode::Apply
        } else {
            Mode::Plan
        },
        profile: args
            .profile
            .clone()
            .unwrap_or_else(|| "default".to_string()),
        lock_label: "permissions push",
    };
    let mut notices = commands::Notices::default();
    let outcome =
        match commands::permissions::execute(solution, &request, server::Client::new, &mut notices)
        {
            Ok(outcome) => outcome,
            Err(error) => {
                print_notices(&notices);
                eprintln!("twaco: {error}");
                return FAILED;
            }
        };
    print_notices(&notices);
    let report = &outcome.report;
    if args.has("--json") {
        let value = serde_json::json!({
            "applied": report.applied,
            "entities": report.entities,
            "recorded": outcome.recorded,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("permissions report serialises")
        );
    } else {
        let mut counts = Vec::new();
        for status in [
            twaco::core::permissions::Status::Same,
            twaco::core::permissions::Status::Differs,
            twaco::core::permissions::Status::Pushed,
            twaco::core::permissions::Status::NotOnServer,
            twaco::core::permissions::Status::Unmanaged,
            twaco::core::permissions::Status::Failed,
        ] {
            let count = report.count(status);
            if count > 0 {
                counts.push(format!("{count} {}", status.label()));
            }
        }
        println!(
            "{} entit{}: {}",
            report.entities.len(),
            if report.entities.len() == 1 {
                "y"
            } else {
                "ies"
            },
            counts.join(", ")
        );
        for entity in &report.entities {
            if entity.differences.is_empty() && entity.error.is_none() {
                continue;
            }
            println!(
                "{}/{}  {}",
                entity.collection,
                entity.name,
                entity.status.label()
            );
            for difference in &entity.differences {
                println!("  {difference}");
            }
            if let Some(error) = &entity.error {
                println!("  failed: {error}");
            }
        }
        if let Some(recorded) = outcome.recorded {
            println!(
                "baseline recorded for {recorded} pushed entit{} now matching the server",
                if recorded == 1 { "y" } else { "ies" }
            );
        }
        let differs = report.count(twaco::core::permissions::Status::Differs);
        if !push && differs > 0 {
            println!(
                "an import never removes a grant or changes the server's allow/deny; \
                 `twaco permissions push` makes these the repository's"
            );
        } else if push && !report.applied && differs > 0 {
            println!("dry run: nothing was written; pass --apply to push");
        }
    }
    if report.count(twaco::core::permissions::Status::Failed) > 0
        || (report.applied && report.count(twaco::core::permissions::Status::NotOnServer) > 0)
    {
        FAILED
    } else if !push && report.count(twaco::core::permissions::Status::Differs) > 0 {
        DRIFT
    } else {
        OK
    }
}

/// Audit each project's permission policy against its entity XML (`permissions audit`). Exits 1
/// when any finding is an error, like a check.
pub(crate) fn permissions_audit_cmd(solution: &Solution, args: &Args) -> u8 {
    use twaco::core::permissions::audit::Severity;
    let request = commands::permissions::AuditRequest {
        project: args.project.clone(),
        server: args.has("--server").then(|| {
            args.profile
                .clone()
                .unwrap_or_else(|| "default".to_string())
        }),
    };
    let report = match commands::permissions::execute_audit(solution, &request, server::Client::new)
    {
        Ok(report) => report,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let detail = args.has("--detail");
    if args.has("--json") {
        let mut value = serde_json::to_value(&report).expect("audit report serialises");
        if !detail {
            for project in value["projects"].as_array_mut().into_iter().flatten() {
                for finding in project["findings"].as_array_mut().into_iter().flatten() {
                    if let Some(object) = finding.as_object_mut() {
                        object.remove("details");
                    }
                }
            }
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("audit report serialises")
        );
    } else {
        for project in &report.projects {
            let mut counts = Vec::new();
            for severity in [Severity::Error, Severity::Warning, Severity::Note] {
                let count = project.count(severity);
                if count > 0 {
                    counts.push(format!(
                        "{count} {}{}",
                        severity.label(),
                        if count == 1 { "" } else { "s" }
                    ));
                }
            }
            let mode = match &project.helper {
                Some(helper) => format!("helper mode, {helper}"),
                None => "plain mode".to_string(),
            };
            println!(
                "{} ({mode}): {} entities, {}",
                project.project,
                project.entities,
                if counts.is_empty() {
                    "as the policy says".to_string()
                } else {
                    counts.join(", ")
                }
            );
            for finding in &project.findings {
                println!("  {finding}");
                if detail {
                    for line in &finding.details {
                        println!("      {line}");
                    }
                }
            }
        }
        if !report.without_policy.is_empty() {
            println!(
                "without a permissions.toml: {}",
                report.without_policy.join(", ")
            );
        }
        if !detail
            && report
                .projects
                .iter()
                .any(|p| p.findings.iter().any(|f| !f.details.is_empty()))
        {
            println!("--detail lists every grant behind a finding");
        }
    }
    if report.count(Severity::Error) > 0 {
        DRIFT
    } else {
        OK
    }
}

/// Write each project's permission policy into its entity XML (`permissions apply`); a plan
/// unless `--apply`.
pub(crate) fn permissions_apply_cmd(solution: &Solution, args: &Args) -> u8 {
    let request = commands::permissions::ApplyRequest {
        project: args.project.clone(),
        mode: if args.has("--apply") {
            Mode::Apply
        } else {
            Mode::Plan
        },
        lock_label: "permissions apply",
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::permissions::execute_apply(solution, &request, &mut notices) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    print_notices(&notices);
    let detail = args.has("--detail");
    let plan = &outcome.plan;
    if args.has("--json") {
        let mut value = serde_json::to_value(plan).expect("apply plan serialises");
        if !detail {
            for project in value["projects"].as_array_mut().into_iter().flatten() {
                for change in project["changes"].as_array_mut().into_iter().flatten() {
                    if let Some(object) = change.as_object_mut() {
                        object.remove("details");
                    }
                }
                for finding in project["remaining"].as_array_mut().into_iter().flatten() {
                    if let Some(object) = finding.as_object_mut() {
                        object.remove("details");
                    }
                }
            }
        }
        value["applied"] = serde_json::json!(outcome.applied);
        println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("apply plan serialises")
        );
        return if plan.remaining_errors() > 0 {
            DRIFT
        } else {
            OK
        };
    }
    for project in &plan.projects {
        let mode = match &project.helper {
            Some(helper) => format!("helper mode, {helper}"),
            None => "plain mode".to_string(),
        };
        let (added, removed) = project
            .changes
            .iter()
            .fold((0, 0), |(a, r), c| (a + c.added, r + c.removed));
        if project.changes.is_empty() {
            println!(
                "{} ({mode}): the entity XML is as the policy says",
                project.project
            );
        } else {
            println!(
                "{} ({mode}): {} file{}, {added} grant{} added, {removed} removed",
                project.project,
                project.changes.len(),
                if project.changes.len() == 1 { "" } else { "s" },
                if added == 1 { "" } else { "s" },
            );
        }
        for change in &project.changes {
            let mut sets: Vec<&str> = change.sets.iter().map(|kind| kind.label()).collect();
            if change.helper {
                sets.push(if change.entity.starts_with("DataShapes/") {
                    "helper columns"
                } else {
                    "helper tables"
                });
            }
            println!(
                "  {}  {}  +{} -{}",
                change.entity,
                sets.join(", "),
                change.added,
                change.removed
            );
            if detail {
                for line in &change.details {
                    println!("      {line}");
                }
            }
        }
        if !project.remaining.is_empty() {
            println!("  left for you (`permissions audit` explains):");
            for finding in &project.remaining {
                println!("    {finding}");
            }
        }
    }
    let changed = plan.changes().count();
    if changed > 0 {
        if outcome.applied {
            println!(
                "wrote {changed} file{}; deploy them, then `twaco permissions push` removes on the server what an import cannot",
                if changed == 1 { "" } else { "s" }
            );
        } else {
            println!("dry run: nothing was written; pass --apply to write");
        }
    }
    if plan.remaining_errors() > 0 {
        DRIFT
    } else {
        OK
    }
}

/// Add the policies' platform grants and memberships the server lacks (`permissions push
/// --platform`); a plan unless `--apply`. Exits 2 when anything failed.
fn permissions_platform_cmd(solution: &Solution, args: &Args) -> u8 {
    use twaco::core::permissions::platform::State;
    let request = commands::permissions::PlatformRequest {
        project: args.project.clone(),
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
    let report =
        match commands::permissions::execute_platform(solution, &request, server::Client::new) {
            Ok(report) => report,
            Err(error) => {
                eprintln!("twaco: {error}");
                return FAILED;
            }
        };
    if args.has("--json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("platform report serialises")
        );
    } else {
        let counts: Vec<String> = [
            State::Present,
            State::Missing,
            State::Added,
            State::Skipped,
            State::Failed,
        ]
        .into_iter()
        .filter(|state| report.count(*state) > 0)
        .map(|state| format!("{} {}", report.count(state), state.label()))
        .collect();
        println!(
            "{} platform grant(s) and membership(s): {}",
            report.items.len(),
            if counts.is_empty() {
                "none in any policy".to_string()
            } else {
                counts.join(", ")
            }
        );
        for item in &report.items {
            if item.state == State::Present {
                continue;
            }
            print!(
                "  {:<8} {} {}: {}",
                item.state.label(),
                item.entity,
                item.what,
                item.group
            );
            match &item.error {
                Some(error) => println!(" ({error})"),
                None => println!(),
            }
        }
        if !report.applied && report.count(State::Missing) > 0 {
            println!("dry run: nothing was written; pass --apply to add what is missing (nothing is ever removed)");
        }
    }
    if report.count(State::Failed) > 0 {
        FAILED
    } else {
        OK
    }
}

/// Draft a permissions.toml for each project without one (`permissions init`); prints the drafts
/// unless `--apply` writes them.
pub(crate) fn permissions_init_cmd(solution: &Solution, args: &Args) -> u8 {
    let request = commands::permissions::InitRequest {
        project: args.project.clone(),
        from_helper: args.has("--from-helper"),
        mode: if args.has("--apply") {
            Mode::Apply
        } else {
            Mode::Plan
        },
        lock_label: "permissions init",
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::permissions::execute_init(solution, &request, &mut notices) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    print_notices(&notices);
    if args.has("--json") {
        let drafts: Vec<serde_json::Value> = outcome
            .drafts
            .iter()
            .map(|draft| {
                serde_json::json!({
                    "project": draft.project,
                    "path": draft.path.display().to_string(),
                    "source": draft.source,
                    "text": draft.text,
                    "notes": draft.notes,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "written": outcome.written,
                "drafts": drafts,
            }))
            .expect("drafts serialise")
        );
        return OK;
    }
    for draft in &outcome.drafts {
        if outcome.written {
            println!("wrote {} (from {})", draft.path.display(), draft.source);
        } else {
            println!("# {} would be:\n{}", draft.path.display(), draft.text);
        }
        for note in &draft.notes {
            println!("  note: {note}");
        }
    }
    if outcome.written {
        println!(
            "`twaco permissions audit` checks it; `permissions apply` should have nothing to do"
        );
    } else if !outcome.drafts.is_empty() {
        println!("dry run: nothing was written; pass --apply to write the drafts");
    }
    OK
}
