use super::flags::{Apply, Detail, Json, OnlyProjects, Profile, Project, Selection};
use super::style;
use crate::{print_notices, DRIFT, FAILED, OK};
use twaco::core::commands::{self, Mode};
use twaco::core::config::Solution;
use twaco::core::{deploy, profile, push, server, types, workflow};

/// `twaco types [--platform | --check [--json]]`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Types {
    #[arg(long = "platform")]
    pub(crate) platform: bool,
    #[arg(long = "check")]
    pub(crate) check: bool,
    #[command(flatten)]
    pub(crate) json: Json,
    #[command(flatten)]
    pub(crate) profile: Profile,
}

pub(crate) fn types_cmd(solution: &Solution, args: &Types) -> u8 {
    let action = if args.check && args.platform {
        commands::types::TypesAction::Invalid(
            "--check and --platform cannot be used together".to_string(),
        )
    } else if args.json.json && !args.check {
        commands::types::TypesAction::Invalid("--json requires --check".to_string())
    } else if args.platform {
        commands::types::TypesAction::Platform
    } else if args.check {
        commands::types::TypesAction::Check
    } else {
        commands::types::TypesAction::Generate
    };
    let request = commands::types::TypesRequest {
        action,
        profile: args.profile.name().to_string(),
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
                eprintln!("{} skipped {skipped}", style::prefix());
            }
            println!(
                "fetched {} templates, {} shapes and {} resources from the server into .twaco/platform.json",
                outcome.templates, outcome.shapes, outcome.resources
            );
            OK
        }
        Ok(commands::types::TypesOutcome::Checked(outcome)) => {
            for skipped in &outcome.declarations.skipped {
                eprintln!("{} skipped {skipped}", style::prefix());
            }
            for finding in &outcome.findings {
                if args.json.json {
                    println!("{}", types::finding_json(finding));
                } else {
                    println!(
                        "{}:{}:{}: TS{} {}",
                        finding.file, finding.line, finding.column, finding.code, finding.message
                    );
                }
            }
            if args.json.json {
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
                eprintln!("{} skipped {skipped}", style::prefix());
            }
            println!(
                "typed {} entities, {} DataShapes and {} services ({} files written)",
                outcome.entities, outcome.data_shapes, outcome.services, outcome.files_written
            );
            if !outcome.gitignore_covers_types {
                eprintln!(
                    "{} note: add `.twaco/types/`, `**/services/*/jsconfig.json`, and \
                     `**/services/*/twaco-globals.d.ts` to the solution root's .gitignore",
                    style::prefix()
                );
            }
            OK
        }
        Err(error) => {
            eprintln!("{} types: {error}", style::prefix());
            FAILED
        }
    }
}

pub(crate) fn print_types_refresh(refresh: &types::Refresh) {
    if let Some(files) = refresh.files_written {
        println!("types: refreshed ({files} files written)");
    }
    if let Some(warning) = &refresh.warning {
        eprintln!("{} warning: types: {warning}", style::prefix());
    }
}

/// `twaco extract (<entity>... | --all) [--project <name>]`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Extract {
    #[command(flatten)]
    pub(crate) selection: Selection,
    #[command(flatten)]
    pub(crate) project: Project,
}

pub(crate) fn extract(solution: &Solution, args: &Extract) -> u8 {
    let request = commands::extract::ExtractRequest {
        target: commands::extract::ExtractTarget {
            project: args.project.project.clone(),
            entities: args.selection.operands.clone(),
            all: args.selection.all,
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
            eprintln!("{} {error}", style::prefix());
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
        eprintln!("{} {} file(s) failed", style::prefix(), outcome.failed);
        return FAILED;
    }
    OK
}

/// `twaco sync (<entity>... | --all) [--check] [--allow-add-remove] [--relayout]`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Sync {
    #[command(flatten)]
    pub(crate) selection: Selection,
    #[arg(long = "check")]
    pub(crate) check: bool,
    #[arg(long = "allow-add-remove")]
    pub(crate) allow_add_remove: bool,
    #[arg(long = "relayout")]
    pub(crate) relayout: bool,
    #[command(flatten)]
    pub(crate) project: Project,
}

pub(crate) fn sync_cmd(solution: &Solution, args: &Sync) -> u8 {
    let check = args.check;
    let request = commands::sync::SyncRequest {
        target: commands::sync::SyncTarget {
            project: args.project.project.clone(),
            entities: args.selection.operands.clone(),
            all: args.selection.all,
            reject_entities_with_all: false,
            missing_target: "name an entity, or pass --all",
        },
        mode: if check { Mode::Plan } else { Mode::Apply },
        allow_structural: args.allow_add_remove,
        relayout: args.relayout,
        lock_label: "sync",
    };
    let mut notices = commands::Notices::default();
    let result = commands::sync::execute(solution, &request, &mut notices);
    print_notices(&notices);
    let outcome = match result {
        Ok(outcome) => outcome.report,
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_log(&outcome.log);
    print_types_refresh(&outcome.types);

    if outcome.failed > 0 {
        eprintln!("{} {} file(s) failed", style::prefix(), outcome.failed);
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
            workflow::Line::Error(text) => eprintln!("{} {text}", style::prefix()),
        }
    }
}

/// `twaco fmt [--check]`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Fmt {
    #[arg(long = "check")]
    pub(crate) check: bool,
}

pub(crate) fn fmt(solution: &Solution, args: &Fmt) -> u8 {
    let check = args.check;
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
            eprintln!("{} {error}", style::prefix());
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
        eprintln!("{} {} script(s) failed", style::prefix(), outcome.failed);
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

/// Print the one-line result of a gate. Pad the plain word first, then style it.
fn print_gate_line(gate: &twaco::core::check::GateResult) {
    if let Some(why) = &gate.skipped {
        println!(
            "  {} {:<14} {why}",
            style::ok(&format!("{:<7}", "skip")),
            gate.name
        );
        return;
    }
    match &gate.broken {
        Some(why) => println!(
            "  {} {:<14} {why}",
            style::fail(&format!("{:<7}", "BROKEN")),
            gate.name
        ),
        None if gate.findings.is_empty() => println!(
            "  {} {:<14} {} examined",
            style::ok(&format!("{:<7}", "ok")),
            gate.name,
            gate.examined
        ),
        // A check that reports without blocking says so, rather than reading as a failure
        // someone has to chase.
        None => {
            let (word, paint): (_, fn(&str) -> String) = if gate.gates_the_run {
                ("FAIL", style::fail)
            } else {
                ("warn", style::warn)
            };
            println!(
                "  {} {:<14} {} finding(s) in {} examined",
                paint(&format!("{word:<7}")),
                gate.name,
                gate.findings.len(),
                gate.examined
            );
        }
    }
}

/// `twaco check [--detail] [--live]`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Check {
    #[command(flatten)]
    pub(crate) detail: Detail,
    #[arg(long = "live")]
    pub(crate) live: bool,
    #[command(flatten)]
    pub(crate) profile: Profile,
}

/// The gate: every built-in check, then every one the solution declares.
pub(crate) fn check(solution: &Solution, args: &Check) -> u8 {
    let mut report = twaco::core::check::run(solution);
    if args.live || solution.gates.live {
        // Credentials are read only here, so the offline gates never need a profile.
        let profile_name = args.profile.name();
        let client = profile::load(&solution.root, profile_name)
            .map(server::Client::new)
            .map_err(|error| error.to_string());
        let checker = client
            .as_ref()
            .map(|client| client as &dyn twaco::core::check::ScriptChecker)
            .map_err(Clone::clone);
        report
            .gates
            .push(twaco::core::check::live_parse_with_progress(
                solution,
                checker,
                &super::progress::reporter(),
            ));
    }
    // Summary by default, detail on request.
    let detail = args.detail.detail;

    for gate in &report.gates {
        print_gate_line(gate);
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
fn print_gate_lines(report: &twaco::core::check::CheckReport) {
    for gate in &report.gates {
        print_gate_line(gate);
    }
    println!();
}

/// What each gate that stopped a deploy found, so the deploy says why: a gate that read the
/// server cannot be reproduced by `twaco check`.
fn print_blocking(report: &twaco::core::check::CheckReport) {
    for gate in report.blocking(8) {
        let name = gate["gate"].as_str().unwrap_or_default();
        if let Some(why) = gate["broken"].as_str() {
            println!("  {name}: could not run: {why}");
        }
        for finding in gate["first"].as_array().into_iter().flatten() {
            println!("  {}", finding.as_str().unwrap_or_default());
        }
        let total = gate["findings"].as_u64().unwrap_or(0);
        if total > 8 {
            println!(
                "  {name}: {} more; `twaco check --detail` lists every offline one",
                total - 8
            );
        }
    }
}

fn print_check_report(report: &twaco::core::check::CheckReport, detail: bool) {
    for gate in &report.gates {
        print_gate_line(gate);
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

/// `twaco bundle [--backend-only] [--handoff <name>] [--check]`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Bundle {
    #[arg(long = "backend-only")]
    pub(crate) backend_only: bool,
    #[arg(long = "check")]
    pub(crate) check: bool,
    #[arg(long = "handoff", value_name = "name")]
    pub(crate) handoff: Option<String>,
    /// Accepted, and changes nothing: the bundle is a local file, written unless `--check`.
    #[command(flatten)]
    pub(crate) apply: Apply,
}

/// Assemble one importable document from the split entity files.
pub(crate) fn bundle(solution: &Solution, args: &Bundle) -> u8 {
    let backend_only = args.backend_only;
    let request = commands::bundle::BundleRequest {
        backend_only,
        handoff: args.handoff.clone(),
        mode: if args.check { Mode::Plan } else { Mode::Apply },
        lock_label: "bundle",
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::bundle::execute(solution, &request, &mut notices) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
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

/// `twaco deploy [--apply] [--force] [--only <entity>]... [--only-projects <a,b>] ...`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Deploy {
    #[command(flatten)]
    pub(crate) json: Json,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[arg(long = "force")]
    pub(crate) force: bool,
    #[arg(long = "backend-only")]
    pub(crate) backend_only: bool,
    #[arg(long = "only", value_name = "entity")]
    pub(crate) only: Vec<String>,
    #[command(flatten)]
    pub(crate) only_projects: OnlyProjects,
    #[arg(long = "skip-checks")]
    pub(crate) skip_checks: bool,
    #[arg(long = "no-backup")]
    pub(crate) no_backup: bool,
    #[command(flatten)]
    pub(crate) profile: Profile,
}

/// Deploy through import, read-back, configured service calls, and a post-service re-read.
pub(crate) fn deploy_cmd(solution: &Solution, args: &Deploy) -> u8 {
    let apply = args.apply.apply;
    let force = args.force;
    let request = commands::deploy::DeployRequest {
        mode: if apply { Mode::Apply } else { Mode::Plan },
        force,
        backup: !args.no_backup,
        skip_checks: args.skip_checks,
        // clap refused a list that names no project.
        only_projects: args.only_projects.projects(),
        only: args.only.clone(),
        backend_only: args.backend_only,
        profile: args.profile.name().to_string(),
        lock_label: "deploy",
    };
    let mut notices = commands::Notices::default();
    let progress = super::progress::reporter();
    let result = commands::deploy::execute(
        solution,
        &request,
        server::Client::new,
        &mut notices,
        &progress,
    );
    if args.json.json {
        print_notices(&notices);
        let (value, code) = deploy_json(result, apply);
        println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("deploy result is JSON")
        );
        return code;
    }
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            if let Some(gates) = error.gates() {
                println!("gates:");
                print_check_report(gates, false);
                println!();
            }
            if matches!(error, commands::deploy::DeployCommandError::Backup { .. }) {
                eprintln!(
                    "{} {error} (--no-backup deploys without one)",
                    style::prefix()
                );
                return FAILED;
            }
            if let Some(dir) = error.backup() {
                println!(
                    "the server's copies of the entities --force overwrites were saved to {dir}"
                );
            }
            return match error {
                commands::deploy::DeployCommandError::Deploy {
                    why,
                    network_changes,
                    ..
                } => match *why {
                    deploy::DeployError::ParseFailed(failures) => {
                        for failure in failures {
                            eprintln!(
                                "{} {}/{} {}:{} {}",
                                style::prefix(),
                                failure.entity,
                                failure.service,
                                failure.line,
                                failure.column,
                                failure.message
                            );
                        }
                        eprintln!("{} live parse failed; no import was sent", style::prefix());
                        FAILED
                    }
                    deploy::DeployError::Conflicts(conflicts) => {
                        print_network_changes(&network_changes);
                        for conflict in conflicts {
                            let deploy::EntityPlan {
                                collection,
                                name,
                                decision,
                                ..
                            } = conflict;
                            if let push::Decision::Refuse(reason) = decision {
                                eprintln!(
                                    "{} {collection}/{name}: refused: {reason}",
                                    style::prefix()
                                );
                            }
                        }
                        eprintln!(
                            "{} nothing was imported; pass --force to overwrite these changes",
                            style::prefix()
                        );
                        DRIFT
                    }
                    deploy::DeployError::NotKept(report) => {
                        print_deploy_report(&report, true, force);
                        for item in &report.not_kept {
                            eprintln!(
                                "{} {}/{}: not kept (sent {}, read back {}{})",
                                style::prefix(),
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
                                        "{} {}/{}: only its permissions differ, and an import never removes a grant or changes the server's allow/deny; `twaco permissions diff {}` shows them, `twaco permissions push {} --apply` makes them the repository's", style::prefix(),
                                        item.collection, item.name, item.name, item.name
                                    );
                            }
                        }
                        FAILED
                    }
                    why => {
                        eprintln!("{} {why}", style::prefix());
                        FAILED
                    }
                },
                error => {
                    eprintln!("{} {error}", style::prefix());
                    FAILED
                }
            };
        }
    };
    print_notices(&notices);
    match outcome {
        commands::deploy::DeployOutcome::GatesBlocked { report, .. } => {
            println!("gates:");
            print_gate_lines(&report);
            print_blocking(&report);
            eprintln!("{} gates block deploy", style::prefix());
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
            localization,
            ..
        } => {
            if let Some(gates) = gates {
                println!("gates:");
                print_check_report(&gates, false);
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
            if let Some(pushed) = localization {
                print_deploy_localization(&pushed);
            }
            OK
        }
    }
}

fn deploy_json(
    result: Result<commands::deploy::DeployOutcome, commands::deploy::DeployCommandError>,
    apply: bool,
) -> (serde_json::Value, u8) {
    use serde_json::json;
    match result {
        Ok(commands::deploy::DeployOutcome::Complete {
            report,
            notes,
            backup,
            localization,
            ..
        }) => {
            let plans: Vec<_> = report.plans.iter().map(|p| json!({"entity": format!("{}/{}", p.collection, p.name), "decision": format!("{:?}", p.decision), "server_owned": p.server_owned})).collect();
            (
                json!({"ok": true, "dry_run": !apply, "projects": notes, "scripts_parsed": report.scripts_checked, "plans": plans,
                "server_owned": report.plans.iter().filter(|p| !p.server_owned.is_empty()).map(|p| json!({"entity": format!("{}/{}", p.collection, p.name), "parts": p.server_owned})).collect::<Vec<_>>(),
                "imported": report.imported, "kept": report.kept.len(), "backup": backup,
                "calls": report.calls.iter().map(|c| json!({"project": c.project, "call": c.call.to_string(), "skipped": c.skipped})).collect::<Vec<_>>(),
                "localization": localization.map(|l| l.tables.iter().map(|t| json!({"table": t.table, "create": t.create, "set": t.set.len(), "imported": l.applied})).collect::<Vec<_>>()),
                "network_connections": report.network_changes.iter().map(|n| json!({"name": n.name, "adds": n.adds, "moves": n.moves, "removes": n.removes})).collect::<Vec<_>>() }),
                OK,
            )
        }
        Ok(commands::deploy::DeployOutcome::GatesBlocked { report, .. }) => (
            json!({"ok":false,"stage":"gates","findings":report.findings(),"broken":report.broken(),"blocking_gates":report.blocking(8)}),
            if report.broken() > 0 { FAILED } else { DRIFT },
        ),
        Err(error) => {
            let mut value = json!({"ok":false,"error":error.to_string(),"backup":error.backup()});
            let code = if let commands::deploy::DeployCommandError::Deploy { why, .. } = &error {
                if let deploy::DeployError::Conflicts(plans) = why.as_ref() {
                    value["refused"] = json!(plans.iter().map(|p| json!({"entity":format!("{}/{}",p.collection,p.name),"decision":format!("{:?}",p.decision),"server_owned":p.server_owned})).collect::<Vec<_>>());
                    DRIFT
                } else {
                    FAILED
                }
            } else {
                FAILED
            };
            (value, code)
        }
    }
}

/// The localization tables a whole-solution deploy carries.
fn print_deploy_localization(pushed: &twaco::core::localization::Pushed) {
    if pushed.tables.is_empty() {
        println!("localization: every table matches the server");
    }
    for table in &pushed.tables {
        let what = format!(
            "{}{} token(s) to set",
            if table.create { "new table, " } else { "" },
            table.set.len()
        );
        if pushed.applied {
            println!(
                "localization {}: {} ({what})",
                table.table,
                style::ok("imported")
            );
        } else {
            println!("localization {}: would import ({what})", table.table);
        }
    }
}

fn print_network_changes(changes: &[deploy::NetworkChange]) {
    for change in changes {
        let mut line = format!(
            "Network {}: import adds {} and moves {} connection(s)",
            change.name, change.adds, change.moves
        );
        if change.removes != 0 {
            line.push_str(&format!(
                ", {} {} server connection(s) {}",
                style::warn("REMOVES"),
                change.removes,
                style::warn("(data loss)")
            ));
        }
        println!("{line}");
    }
}

fn print_deploy_report(report: &deploy::Report, apply: bool, force: bool) {
    println!(
        "live parse: {} script service(s) passed",
        report.scripts_checked
    );
    print_network_changes(&report.network_changes);
    for plan in &report.plans {
        let label = format!("{}/{}", plan.collection, plan.name);
        if !plan.server_owned.is_empty() {
            println!(
                "  {label}: keeps the server's {}",
                plan.server_owned.join(", ")
            );
        }
        if !plan.cleared.is_empty() {
            println!(
                "  {label}: the import clears the server's {} (an encrypted value cannot be \
                 carried); a deploy hook must set it again",
                plan.cleared.join(", ")
            );
        }
        match &plan.decision {
            push::Decision::AlreadyThere => println!("  {label}: nothing to push"),
            push::Decision::Create => println!("  {label}: would create"),
            push::Decision::Update => {
                println!("  {label}: would update; server unchanged since baseline")
            }
            push::Decision::Refuse(reason) if force => {
                println!("  {label}: would overwrite with --force ({reason})")
            }
            push::Decision::Refuse(reason) => {
                println!("  {label}: {} ({reason})", style::warn("would refuse"))
            }
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
            println!("project {project}: {}", style::ok("imported"));
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
