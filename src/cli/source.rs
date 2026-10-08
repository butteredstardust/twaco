use super::super::*;

pub(crate) fn types_cmd(solution: &Solution, args: &Args) -> u8 {
    let action = if args.has("--check") && args.has("--platform") {
        commands::types::TypesAction::Invalid(
            "--check and --platform cannot be used together".to_string(),
        )
    } else if args.has("--json") && !args.has("--check") {
        commands::types::TypesAction::Invalid("--json requires --check".to_string())
    } else if args.has("--platform") {
        commands::types::TypesAction::Platform
    } else if args.has("--check") {
        commands::types::TypesAction::Check
    } else {
        commands::types::TypesAction::Generate
    };
    let request = commands::types::TypesRequest {
        action,
        profile: args
            .profile
            .clone()
            .unwrap_or_else(|| "default".to_string()),
        lock_label: "types",
    };
    let mut notices = commands::Notices::default();
    let progress = super::progress::reporter();
    let result = commands::types::execute(
        solution,
        &request,
        server::Client::new,
        None,
        &mut notices,
        &progress,
    );
    print_notices(&notices);
    match result {
        Ok(commands::types::TypesOutcome::Platform(outcome)) => {
            for skipped in outcome.skipped.iter().chain(&outcome.types.skipped) {
                eprintln!("twaco: skipped {skipped}");
            }
            println!(
                "fetched {} templates, {} shapes and {} resources from the server into .twaco/platform.json",
                outcome.templates, outcome.shapes, outcome.resources
            );
            OK
        }
        Ok(commands::types::TypesOutcome::Checked(outcome)) => {
            for skipped in &outcome.declarations.skipped {
                eprintln!("twaco: skipped {skipped}");
            }
            for finding in &outcome.findings {
                if args.has("--json") {
                    println!("{}", types::finding_json(finding));
                } else {
                    println!(
                        "{}:{}:{}: TS{} {}",
                        finding.file, finding.line, finding.column, finding.code, finding.message
                    );
                }
            }
            if args.has("--json") {
                eprintln!("{}", types::check_summary(&outcome));
            } else {
                println!("{}", types::check_summary(&outcome));
            }
            if outcome.findings.is_empty() {
                OK
            } else {
                DRIFT
            }
        }
        Ok(commands::types::TypesOutcome::Generated(outcome)) => {
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

pub(crate) fn print_types_refresh(refresh: &types::Refresh) {
    if let Some(files) = refresh.files_written {
        println!("types: refreshed ({files} files written)");
    }
    if let Some(warning) = &refresh.warning {
        eprintln!("twaco: warning: types: {warning}");
    }
}

pub(crate) fn extract(solution: &Solution, args: &Args) -> u8 {
    let request = commands::extract::ExtractRequest {
        target: commands::extract::ExtractTarget {
            project: args.project.clone(),
            entities: args.names.clone(),
            all: args.has("--all"),
            reject_entities_with_all: false,
            missing_target: "name an entity, or pass --all",
        },
        lock_label: "extract",
    };
    let mut notices = commands::Notices::default();
    let result = commands::extract::execute(solution, &request, &mut notices);
    print_notices(&notices);
    let outcome = match result {
        Ok(outcome) => outcome.report,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    print_log(&outcome.log);
    print_types_refresh(&outcome.types);
    // "part" rather than "service": an entity yields services or fields depending on what
    // it is, and one counter covers both.
    println!(
        "{} part(s) from {} entity file(s)",
        outcome.written, outcome.entities
    );
    if outcome.failed > 0 {
        eprintln!("twaco: {} file(s) failed", outcome.failed);
        return FAILED;
    }
    OK
}

pub(crate) fn sync_cmd(solution: &Solution, args: &Args) -> u8 {
    let check = args.has("--check");
    let request = commands::sync::SyncRequest {
        target: commands::sync::SyncTarget {
            project: args.project.clone(),
            entities: args.names.clone(),
            all: args.has("--all"),
            reject_entities_with_all: false,
            missing_target: "name an entity, or pass --all",
        },
        mode: if check { Mode::Plan } else { Mode::Apply },
        allow_structural: args.has("--allow-add-remove"),
        relayout: args.has("--relayout"),
        lock_label: "sync",
    };
    let mut notices = commands::Notices::default();
    let result = commands::sync::execute(solution, &request, &mut notices);
    print_notices(&notices);
    let outcome = match result {
        Ok(outcome) => outcome.report,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
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
        println!(
            "{} of {} entity file(s) would change",
            outcome.changed, outcome.checked
        );
        DRIFT
    } else {
        println!(
            "{} of {} entity file(s) updated",
            outcome.changed, outcome.checked
        );
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

pub(crate) fn fmt(solution: &Solution, args: &Args) -> u8 {
    let check = args.has("--check");
    let request = commands::fmt::FmtRequest {
        mode: if check { Mode::Plan } else { Mode::Apply },
        lock_label: "fmt",
    };
    let mut notices = commands::Notices::default();
    let result = commands::fmt::execute(solution, &request, &mut notices);
    print_notices(&notices);
    let outcome = match result {
        Ok(outcome) => outcome.report,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    if outcome.files == 0 {
        println!("no service scripts under {}", solution.src_root().display());
        return OK;
    }
    print_log(&outcome.log);
    for path in &outcome.changed {
        println!(
            "{} {}",
            if check {
                "would reformat"
            } else {
                "reformatted"
            },
            path.display()
        );
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
        if check {
            "need formatting"
        } else {
            "reformatted"
        }
    );
    if check {
        DRIFT
    } else {
        OK
    }
}

/// The gate: every built-in check, then every one the solution declares.
pub(crate) fn check(solution: &Solution, args: &Args) -> u8 {
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
        report
            .gates
            .push(twaco::core::check::live_parse(solution, checker));
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
        report
            .gates
            .iter()
            .filter(|g| !g.findings.is_empty())
            .count(),
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

/// Print a gate report in the command-line form shared by `check` and deploy's gate outcome.
fn print_check_report(report: &twaco::core::check::CheckReport, detail: bool) {
    for gate in &report.gates {
        match &gate.broken {
            Some(why) => println!("  BROKEN  {:<14} {why}", gate.name),
            None if gate.findings.is_empty() => {
                println!("  ok      {:<14} {} examined", gate.name, gate.examined)
            }
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
    } else {
        println!(
            "{} finding(s) across {} gate(s); {} gate(s) could not run",
            report.findings(),
            report
                .gates
                .iter()
                .filter(|gate| !gate.findings.is_empty())
                .count(),
            report.broken()
        );
        if !detail {
            println!("run `twaco check --detail` to see them");
        }
        if !report.blocks() {
            println!("nothing found blocks the run");
        }
    }
}

/// Assemble one importable document from the split entity files.
pub(crate) fn bundle(solution: &Solution, args: &Args) -> u8 {
    let backend_only = args.has("--backend-only");
    let request = commands::bundle::BundleRequest {
        backend_only,
        mode: if args.has("--check") {
            Mode::Plan
        } else {
            Mode::Apply
        },
        lock_label: "bundle",
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::bundle::execute(solution, &request, &mut notices) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    print_notices(&notices);
    // A reference to something this repository owns but this bundle leaves out. Scoped to the
    // selection, so a backend-only build reports what it dropped rather than what it kept.
    for dangling in twaco::core::bundle::dangling_in_selection(solution, backend_only) {
        println!("  note: {dangling}");
    }
    match outcome {
        commands::bundle::BundleOutcome::Current { target, bundle, .. } => {
            println!(
                "{} is current: {} entities from {} file(s)",
                target.display(),
                bundle.entities.len(),
                bundle.files
            );
            OK
        }
        commands::bundle::BundleOutcome::OutOfDate { target, .. } => {
            println!("{} is out of date; run `twaco bundle`", target.display());
            DRIFT
        }
        commands::bundle::BundleOutcome::Missing { target, .. } => {
            println!(
                "{} has not been built; run `twaco bundle`",
                target.display()
            );
            DRIFT
        }
        commands::bundle::BundleOutcome::Written { target, bundle, .. } => {
            println!(
                "wrote {}: {} entities from {} file(s), {} bytes",
                target.display(),
                bundle.entities.len(),
                bundle.files,
                bundle.bytes.len()
            );
            OK
        }
    }
}

/// Deploy through import, read-back, configured service calls, and a post-service re-read.
pub(crate) fn deploy_cmd(solution: &Solution, args: &Args) -> u8 {
    if !args.names.is_empty() {
        eprintln!("twaco: deploy takes no positional names; use --only <entity>");
        return FAILED;
    }

    let apply = args.has("--apply");
    let force = args.has("--force");
    let request = commands::deploy::DeployRequest {
        mode: if apply { Mode::Apply } else { Mode::Plan },
        force,
        backup: !args.has("--no-backup"),
        skip_checks: args.has("--skip-checks"),
        only_projects: args.only_projects.clone(),
        only: args.only.clone(),
        backend_only: args.has("--backend-only"),
        profile: args
            .profile
            .clone()
            .unwrap_or_else(|| "default".to_string()),
        lock_label: "deploy",
    };
    let mut notices = commands::Notices::default();
    let progress = super::progress::reporter();
    let outcome = match commands::deploy::execute(
        solution,
        &request,
        server::Client::new,
        &mut notices,
        &progress,
    ) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            if let Some(gates) = error.gates() {
                println!("offline gates:");
                print_check_report(gates, args.has("--detail"));
                println!();
            }
            if matches!(error, commands::deploy::DeployCommandError::Backup { .. }) {
                eprintln!("twaco: {error} (--no-backup deploys without one)");
                return FAILED;
            }
            if let Some(dir) = error.backup() {
                println!(
                    "the server's copies of the entities --force overwrites were saved to {dir}"
                );
            }
            return match error {
                commands::deploy::DeployCommandError::Deploy { why, .. } => {
                    match *why {
                        deploy::DeployError::ParseFailed(failures) => {
                            for failure in failures {
                                eprintln!(
                                    "twaco: {}/{} {}:{} {}",
                                    failure.entity,
                                    failure.service,
                                    failure.line,
                                    failure.column,
                                    failure.message
                                );
                            }
                            eprintln!("twaco: live parse failed; no import was sent");
                            FAILED
                        }
                        deploy::DeployError::Conflicts(conflicts) => {
                            for conflict in conflicts {
                                let deploy::EntityPlan {
                                    collection,
                                    name,
                                    decision,
                                    ..
                                } = conflict;
                                if let push::Decision::Refuse(reason) = decision {
                                    eprintln!("twaco: {collection}/{name}: refused: {reason}");
                                }
                            }
                            eprintln!("twaco: nothing was imported; pass --force to overwrite these changes");
                            DRIFT
                        }
                        deploy::DeployError::NotKept(report) => {
                            print_deploy_report(&report, true, force);
                            for item in &report.not_kept {
                                eprintln!(
                                    "twaco: {}/{}: not kept (sent {}, read back {}{})",
                                    item.collection,
                                    item.name,
                                    item.sent,
                                    item.read_back.as_deref().unwrap_or("nothing"),
                                    item.error
                                        .as_ref()
                                        .map(|why| format!("; {why}"))
                                        .unwrap_or_default()
                                );
                                if item.only_permissions {
                                    eprintln!(
                                        "twaco: {}/{}: only its permissions differ, and an import never removes a grant or changes the server's allow/deny; `twaco permissions diff {}` shows them, `twaco permissions push {} --apply` makes them the repository's",
                                        item.collection, item.name, item.name, item.name
                                    );
                                }
                            }
                            FAILED
                        }
                        why => {
                            eprintln!("twaco: {why}");
                            FAILED
                        }
                    }
                }
                error => {
                    eprintln!("twaco: {error}");
                    FAILED
                }
            };
        }
    };
    print_notices(&notices);
    match outcome {
        commands::deploy::DeployOutcome::GatesBlocked { report, .. } => {
            println!("offline gates:");
            print_check_report(&report, args.has("--detail"));
            eprintln!("twaco: offline gates block deploy");
            if report.broken() > 0 {
                FAILED
            } else {
                DRIFT
            }
        }
        commands::deploy::DeployOutcome::Complete {
            report,
            gates,
            notes,
            backup,
            ..
        } => {
            if let Some(gates) = gates {
                println!("offline gates:");
                print_check_report(&gates, args.has("--detail"));
                println!();
            }
            for note in notes {
                println!("{note}");
            }
            if let Some(dir) = backup {
                println!(
                    "the server's copies of the entities --force overwrites were saved to {dir}"
                );
            }
            print_deploy_report(&report, apply, force);
            OK
        }
    }
}

fn print_deploy_report(report: &deploy::Report, apply: bool, force: bool) {
    println!(
        "live parse: {} script service(s) passed",
        report.scripts_checked
    );
    for plan in &report.plans {
        let label = format!("{}/{}", plan.collection, plan.name);
        match &plan.decision {
            push::Decision::AlreadyThere => println!("  {label}: nothing to push"),
            push::Decision::Create => println!("  {label}: would create"),
            push::Decision::Update => {
                println!("  {label}: would update; server unchanged since baseline")
            }
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
