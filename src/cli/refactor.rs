use super::flags::{Apply, Detail, Json};
use super::source::print_types_refresh;
use super::style;
use crate::{print_notices, DRIFT, FAILED, OK};
use std::path::Path;
use std::path::PathBuf;
use twaco::core::commands::{self, Mode};
use twaco::core::config::Solution;
use twaco::core::{adopt, newblock, relocate, rename, retemplate};

/// `twaco adopt <export> [--entity <name>]... [--base <file|handoff|git rev>] ...`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Adopt {
    #[arg(long = "entity", value_name = "name")]
    pub(crate) entity_filters: Vec<String>,
    #[arg(long = "base", value_name = "file|handoff|git rev")]
    pub(crate) base: Option<String>,
    #[arg(long = "only", value_name = "entity")]
    pub(crate) only: Vec<String>,
    #[arg(long = "take", value_name = "side:name,...")]
    pub(crate) take: Option<String>,
    #[command(flatten)]
    pub(crate) detail: Detail,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(long = "fail-on-revert")]
    pub(crate) fail_on_revert: bool,
    #[arg(long = "suggest-base")]
    pub(crate) suggest_base: bool,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// Compare a designer's export with the solution: what it would revert, and what it would change.
///
/// Read-only. Summary by default; `--detail` lists every differing node, and
/// `--json` gives the same summary as JSON.
pub(crate) fn adopt_cmd(solution: &Solution, args: &Adopt) -> u8 {
    if args.operands.len() != 1 {
        eprintln!(
            "{} adopt needs the path of one <Entities> export",
            style::prefix()
        );
        return FAILED;
    }
    let only_kind = match args.only.as_slice() {
        [] => None,
        [value] => match value.as_str() {
            "ui" => Some(adopt::Kind::Ui),
            "backend" => Some(adopt::Kind::Backend),
            _ => {
                eprintln!(
                    "{} --only needs `ui` or `backend`, not `{value}`",
                    style::prefix()
                );
                return FAILED;
            }
        },
        _ => {
            eprintln!("{} --only needs one of `ui` or `backend`", style::prefix());
            return FAILED;
        }
    };
    let takes = match parse_takes(args.take.as_deref()) {
        Ok(takes) => takes,
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    if args.suggest_base {
        match adopt::Side::from_export(&PathBuf::from(&args.operands[0]))
            .and_then(|export| adopt::base_suggestions(solution, &export))
        {
            Ok(suggestions) if suggestions.is_empty() => {
                if args.json.json {
                    println!("{}", serde_json::json!({ "notes": [adopt::NO_BASE_NOTE] }));
                } else {
                    println!("{}", adopt::NO_BASE_NOTE);
                }
            }
            Ok(suggestions) => {
                if args.json.json {
                    println!("{}", adopt::suggestions_json(&suggestions));
                } else {
                    for suggestion in suggestions {
                        println!(
                            "{}  {}  {}  {}  {}/{}",
                            suggestion.handoff.name,
                            suggestion.handoff.commit.as_deref().unwrap_or("none"),
                            jiff::Timestamp::from_millisecond(suggestion.handoff.created as i64)
                                .map(|at| at.strftime("%Y-%m-%d %H:%M").to_string())
                                .unwrap_or_else(|_| suggestion.handoff.created.to_string()),
                            if suggestion.handoff.dirty {
                                "dirty"
                            } else {
                                "clean"
                            },
                            suggestion.matches,
                            suggestion.total,
                        );
                    }
                }
            }
            Err(error) => {
                eprintln!("{} {error}", style::prefix());
                return FAILED;
            }
        }
        return OK;
    }
    let request = commands::adopt::AdoptRequest {
        export: PathBuf::from(&args.operands[0]),
        only: args.entity_filters.clone(),
        base: args.base.clone(),
        only_kind,
        takes,
        mode: if args.apply.apply {
            Mode::Apply
        } else {
            Mode::Plan
        },
        lock_label: "adopt",
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::adopt::execute(solution, &request, &mut notices) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    let (report, applied) = match outcome {
        commands::adopt::AdoptOutcome::Plan { report, .. } => (report, None),
        commands::adopt::AdoptOutcome::Applied {
            report, outcome, ..
        } => (report, Some(outcome)),
    };
    let reverts = report.reverts().count();

    if args.json.json {
        let services: Vec<serde_json::Value> = report
            .services
            .iter()
            .map(|s| serde_json::json!({ "entity": s.entity, "service": s.service, "generated": s.generated, "change": change_word(s.change), "kind": "backend" }))
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
            "type_changes": report.type_changes().iter().map(|(entity, d)| serde_json::json!({ "entity": entity.path(), "path": d.path, "export": d.export, "repo": d.repo })).collect::<Vec<_>>(),
            "retyped_arguments": report.retyped_arguments.iter().map(|a| serde_json::json!({ "service_entity": a.service_entity, "service": a.service, "parameter": a.parameter, "from": a.from, "to": a.to, "caller": a.caller, "file": solution.relative(&a.file), "line": a.line, "literal": a.literal, "mapped": a.mapped_source(), "why": a.unmappable_why() })).collect::<Vec<_>>(),
            "retyped_defaults": report.retyped_defaults.iter().map(|d| serde_json::json!({ "entity": d.entity, "service": d.service, "parameter": d.parameter, "to": d.to, "default": d.default, "why": d.why })).collect::<Vec<_>>(),
            "identical": report.with_status(adopt::Status::Identical).count(),
            "base": report.base,
            "notes": report.notes,
            "entities": report.entities.iter().map(|e| serde_json::json!({ "entity": e.entity.path(), "change": change_word(e.change), "kind": kind_word(e.kind) })).collect::<Vec<_>>(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&summary).expect("JSON values serialise")
        );
    } else {
        print_adopt_report(solution, &report, args.detail.detail);
    }
    if let Some(outcome) = applied.as_ref() {
        // A conflict, a kept or a skipped entity is reported beside what was written, not as
        // something that was.
        let (left, written): (Vec<&String>, Vec<&String>) =
            outcome.lines.iter().partition(|line| {
                ["conflict ", "kept ", "skipped "]
                    .iter()
                    .any(|word| line.starts_with(word))
            });
        println!("\n=== applied ({}) ===", written.len());
        for line in &written {
            println!("  {line}");
        }
        if !left.is_empty() {
            println!("\n=== not applied ({}) ===", left.len());
            for line in &left {
                println!("  {line}");
            }
        }
        print_types_refresh(&outcome.types);
        let mashup_written = report.entities.iter().any(|entry| {
            entry.entity.collection == "Mashups"
                && outcome
                    .lines
                    .iter()
                    .any(|line| line == &format!("sidecar  {}", entry.entity.name))
        });
        if written.is_empty() {
            println!("\nNothing was adopted.");
        } else if mashup_written {
            println!("\nMashup sidecars may need `twaco sync --all`; backend changes are already folded. Then run `twaco check`.");
        } else {
            println!("\nBackend changes are already folded; run `twaco check`.");
        }
        if reverts > 0 {
            println!(
                "The {reverts} service difference(s) above were not applied; they need a person."
            );
        }
    }
    // Only a retype this apply adopted leaves values for a person to change.
    let unmappable = applied.as_ref().map_or(0, |outcome| {
        outcome.unmapped_arguments(&report).count() + outcome.unmapped_defaults(&report).count()
    });
    if unmappable > 0 {
        eprintln!(
            "{} {unmappable} retyped value(s) remain unmapped",
            style::prefix()
        );
        return DRIFT;
    }
    if args.fail_on_revert && reverts > 0 {
        DRIFT
    } else {
        OK
    }
}

fn parse_takes(value: Option<&str>) -> Result<Vec<adopt::Take>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    value
        .split(',')
        .map(|item| {
            let Some((side, target)) = item.split_once(':') else {
                return Err(format!("bad --take `{item}`; expected `theirs:Entity[.Service]` or `ours:Entity[.Service]`"));
            };
            let side = match side {
                "theirs" => adopt::TakeSide::Theirs,
                "ours" => adopt::TakeSide::Ours,
                _ => return Err(format!("bad --take `{item}`; expected `theirs:Entity[.Service]` or `ours:Entity[.Service]`")),
            };
            if target.is_empty() || target.contains(':') {
                return Err(format!("bad --take `{item}`; expected `theirs:Entity[.Service]` or `ours:Entity[.Service]`"));
            }
            Ok(adopt::Take { side, target: target.to_string() })
        })
        .collect()
}

fn change_word(change: adopt::Change) -> &'static str {
    match change {
        adopt::Change::Same => "same",
        adopt::Change::Stale => "stale",
        adopt::Change::Theirs => "theirs",
        adopt::Change::Conflict => "conflict",
        adopt::Change::Added => "added",
        adopt::Change::WeRemoved => "we_removed",
        adopt::Change::Unknown => "unknown",
    }
}

fn kind_word(kind: adopt::Kind) -> &'static str {
    match kind {
        adopt::Kind::Ui => "ui",
        adopt::Kind::Backend => "backend",
    }
}

/// `twaco handoff list|record <file.xml>... --name <name>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Handoff {
    #[arg(long = "name", value_name = "name")]
    pub(crate) name: Option<String>,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

pub(crate) fn handoff_cmd(solution: &Solution, args: &Handoff) -> u8 {
    let Some(action) = args.operands.first().map(String::as_str) else {
        eprintln!(
            "{} handoff needs `list` or `record <file.xml>... --name <name>`",
            style::prefix()
        );
        return FAILED;
    };
    let request = match action {
        "list" if args.operands.len() == 1 => commands::handoff::HandoffRequest::List,
        "record" => {
            let Some(name) = args.name.as_ref() else {
                eprintln!("{} handoff record needs --name <name>", style::prefix());
                return FAILED;
            };
            let files = args.operands[1..].iter().map(PathBuf::from).collect();
            commands::handoff::HandoffRequest::Record {
                name: name.clone(),
                files,
            }
        }
        _ => {
            eprintln!(
                "{} handoff needs `list` or `record <file.xml>... --name <name>`",
                style::prefix()
            );
            return FAILED;
        }
    };
    let mode = if args.apply.apply {
        Mode::Apply
    } else {
        Mode::Plan
    };
    let mut notices = commands::Notices::default();
    let outcome =
        match commands::handoff::execute(solution, &request, mode, "handoff", &mut notices) {
            Ok(outcome) => outcome,
            Err(error) => {
                print_notices(&notices);
                eprintln!("{} {error}", style::prefix());
                return FAILED;
            }
        };
    print_notices(&notices);
    match outcome {
        commands::handoff::HandoffOutcome::Listed { handoffs, .. } => {
            if handoffs.is_empty() {
                println!("no recorded handoffs");
            }
            for handoff in handoffs {
                println!(
                    "{}  {}  {}  {}  {}",
                    handoff.name,
                    jiff::Timestamp::from_millisecond(handoff.created as i64)
                        .map(|at| at.strftime("%Y-%m-%d %H:%M").to_string())
                        .unwrap_or_else(|_| handoff.created.to_string()),
                    handoff.commit.as_deref().unwrap_or("none"),
                    if handoff.dirty { "dirty" } else { "clean" },
                    handoff.files.join(", ")
                );
            }
        }
        commands::handoff::HandoffOutcome::Planned { name, files, .. } => println!(
            "would record handoff {name}: {}",
            files
                .iter()
                .map(|file| file.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        commands::handoff::HandoffOutcome::Recorded { handoff, .. } => println!(
            "recorded handoff {}: {}",
            handoff.name,
            handoff.files.join(", ")
        ),
    }
    OK
}

/// `twaco rename <kind> ...`.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum Rename {
    Entity(RenameData),
    Prefix(RenameData),
    Field(RenameData),
    Service(RenameArgs),
    Param(RenameArgs),
    Table(RenameArgs),
    Property(RenameArgs),
}

/// What every rename takes.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct RenameArgs {
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[arg(long = "text")]
    pub(crate) text: bool,
    #[command(flatten)]
    pub(crate) detail: Detail,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(long = "skip-checks")]
    pub(crate) skip_checks: bool,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// A rename that can reach stored data: what every rename takes, and the migration script.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct RenameData {
    #[command(flatten)]
    pub(crate) args: RenameArgs,
    #[arg(long = "sql")]
    pub(crate) sql: bool,
    #[arg(long = "sql-dir", value_name = "dir")]
    pub(crate) sql_dir: Option<String>,
    #[arg(long = "no-sql")]
    pub(crate) no_sql: bool,
}

impl Rename {
    /// The command's [`USAGE`](super::spec::USAGE) path, which is also its lock label.
    pub(crate) fn path(&self) -> &'static str {
        match self {
            Rename::Entity(_) => "rename entity",
            Rename::Prefix(_) => "rename prefix",
            Rename::Field(_) => "rename field",
            Rename::Service(_) => "rename service",
            Rename::Param(_) => "rename param",
            Rename::Table(_) => "rename table",
            Rename::Property(_) => "rename property",
        }
    }

    /// What every rename takes.
    pub(crate) fn args(&self) -> &RenameArgs {
        match self {
            Rename::Entity(data) | Rename::Prefix(data) | Rename::Field(data) => &data.args,
            Rename::Service(args)
            | Rename::Param(args)
            | Rename::Table(args)
            | Rename::Property(args) => args,
        }
    }

    /// The migration script flags, for the renames that take them.
    fn database(&self) -> Option<&RenameData> {
        match self {
            Rename::Entity(data) | Rename::Prefix(data) | Rename::Field(data) => Some(data),
            Rename::Service(_) | Rename::Param(_) | Rename::Table(_) | Rename::Property(_) => None,
        }
    }
}

pub(crate) fn rename_cmd(solution: &Solution, command: &Rename) -> u8 {
    let route = command.path();
    let args = command.args();
    let word = route.strip_prefix("rename ").unwrap_or(route);
    let kind = rename::Kind::from_word(word).expect("the route names a rename kind");
    let mut request = match rename::Request::from_names(kind, &args.operands) {
        Ok(request) => request,
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    request.apply = args.apply.apply;
    request.include_outside = args.text;
    request.skip_checks = args.skip_checks;
    request.database = command
        .database()
        .map_or_else(rename::DatabaseFlags::default, |database| {
            rename::DatabaseFlags {
                sql: database.sql,
                no_sql: database.no_sql,
                dir: database.sql_dir.clone(),
            }
        });
    let request = commands::rename::RenameRequest {
        mode: if args.apply.apply {
            Mode::Apply
        } else {
            Mode::Plan
        },
        request,
        date: jiff::Zoned::now().strftime("%Y-%m-%d").to_string(),
        lock_label: route.to_string(),
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::rename::execute(solution, &request, &mut notices) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    let outcome = outcome.outcome();
    if args.json.json {
        print_rename_json(solution, outcome, args.text);
    } else {
        print_rename_outcome(solution, outcome, args.text, args.detail.detail);
    }
    let verification_failed = outcome.verification.as_ref().is_some_and(|verification| {
        !verification.sync_problems.is_empty() || !verification.blocking_gates.is_empty()
    });
    if verification_failed {
        eprintln!("{} the rename is applied, but verification found problems; fix the sync entities and blocking gates listed above", style::prefix());
        FAILED
    } else {
        OK
    }
}

fn print_rename_outcome(
    solution: &Solution,
    outcome: &rename::Outcome,
    include_outside: bool,
    detail: bool,
) {
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
    let review = [
        counts.entity,
        counts.sidecar,
        counts.config,
        counts.outside,
        counts.mashup,
    ]
    .iter()
    .map(|count| count.review)
    .sum::<usize>();
    println!("  review         {review} occurrences left for a person");
    if !plan.named.is_empty() {
        println!(
            "  file names     {} file(s) are named for the old name and were not renamed",
            plan.named.len()
        );
    }
    if !plan.skipped.is_empty() {
        println!(
            "  skipped        {} files that are not text",
            plan.skipped.len()
        );
    }
    if let Some(script) = &outcome.sql {
        let verb = if outcome.applied.is_some() {
            "written to"
        } else {
            "would be written to"
        };
        println!(
            "  database       a migration {verb} {}",
            display_relative(solution, &script.path)
        );
    }

    if detail {
        if let Some(script) = &outcome.sql {
            println!("\n{}", script.text);
        }
        for change in plan.changes.iter().chain(&plan.outside) {
            for finding in &change.findings {
                println!(
                    "{}:{}  {}",
                    display_relative(solution, &change.path),
                    finding.line,
                    finding.excerpt
                );
            }
        }
        for path in &plan.named {
            println!("{}", display_relative(solution, path));
        }
    } else if review > 0 {
        let findings = plan
            .changes
            .iter()
            .chain(&plan.outside)
            .flat_map(|change| {
                change
                    .findings
                    .iter()
                    .filter(|finding| finding.tier == twaco::core::refs::Tier::Review)
                    .map(move |finding| (change, finding))
            })
            .collect::<Vec<_>>();
        for (change, finding) in findings.iter().take(10) {
            println!(
                "{}:{}  {}",
                display_relative(solution, &change.path),
                finding.line,
                finding.excerpt
            );
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
    if matches!(
        plan.spec.kind,
        rename::Kind::Field
            | rename::Kind::Service
            | rename::Kind::Param
            | rename::Kind::Table
            | rename::Kind::Property
    ) {
        println!("Next: twaco check; commit; twaco deploy --apply.");
    } else {
        println!("Next: twaco check; commit; twaco deploy --apply; then delete the old entities from each server.");
    }
}

fn print_rename_json(solution: &Solution, outcome: &rename::Outcome, include_outside: bool) {
    let value = rename::summary_json(solution, outcome, include_outside, usize::MAX);
    println!(
        "{}",
        serde_json::to_string_pretty(&value).expect("rename result serialises")
    );
}

fn print_adopt_report(solution: &Solution, report: &adopt::Report, detail: bool) {
    match &report.base {
        Some(base) => println!("base: {base}"),
        None => println!("base: none"),
    }
    for note in &report.notes {
        println!("note: {note}");
    }
    for (heading, changes) in [
        ("theirs", &[adopt::Change::Theirs][..]),
        ("added", &[adopt::Change::Added][..]),
        ("conflict", &[adopt::Change::Conflict][..]),
        ("unknown", &[adopt::Change::Unknown][..]),
        ("stale", &[adopt::Change::Stale][..]),
        ("we removed", &[adopt::Change::WeRemoved][..]),
    ] {
        let entities: Vec<_> = report
            .entities
            .iter()
            .filter(|entry| changes.contains(&entry.change))
            .collect();
        let services: Vec<_> = report
            .services
            .iter()
            .filter(|service| changes.contains(&service.change))
            .collect();
        if entities.is_empty() && services.is_empty() {
            continue;
        }
        println!("\n=== {heading} ({}) ===", entities.len() + services.len());
        for entry in entities {
            println!(
                "  {}  {}  ({})",
                change_word(entry.change),
                entry.entity.path(),
                kind_word(entry.kind)
            );
        }
        for service in services {
            println!(
                "  {}  {}.{}  (backend)",
                change_word(service.change),
                service.entity,
                service.service
            );
        }
    }
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
        let suffix = if notes.is_empty() {
            String::new()
        } else {
            format!("  ({})", notes.join("; "))
        };
        println!("  {}.{}{suffix}", service.entity, service.service);
        println!("      {}", display_relative(solution, &service.source));
    }
    let reverts = report.reverts().count();
    if reverts > 0 {
        println!(
            "\n  {reverts} service(s) differ and are not marked generated. Each is either the"
        );
        println!(
            "  designer's change to adopt or one of ours they never had: read the diff before"
        );
        println!("  importing, and redeploy the entity afterwards.");
    }
    if !report.unmatched_services.is_empty() {
        let shown: Vec<&str> = report
            .unmatched_services
            .iter()
            .take(5)
            .map(String::as_str)
            .collect();
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
        println!(
            "  {:16} {}{project}",
            entry.entity.collection, entry.entity.name
        );
    }

    println!(
        "\n=== here but not in the export ({}) ===",
        report.absent.len()
    );
    for entity in &report.absent {
        println!("  {:16} {}", entity.collection, entity.name);
    }
    if !report.absent.is_empty() {
        println!(
            "  The export does not say whether these were deleted or never left their server."
        );
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
        let suffix = if extra.is_empty() {
            String::new()
        } else {
            format!("  [{}]", extra.join(", "))
        };
        println!(
            "  {:16} {}  ({} node(s)){suffix}",
            entry.entity.collection,
            entry.entity.name,
            entry.differences.len()
        );
        if detail {
            let side = |value: &Option<String>| match value {
                None => "<absent>".to_string(),
                Some(text) if text.chars().count() > 160 => {
                    format!("{}...", text.chars().take(160).collect::<String>())
                }
                Some(text) => text.clone(),
            };
            for difference in &entry.differences {
                println!("    {}", difference.path);
                println!("      export: {}", side(&difference.export));
                println!("      repo  : {}", side(&difference.repo));
            }
        }
    }

    let retyped = report.type_changes();
    if !retyped.is_empty() {
        println!("\n=== base-type changes ({}) ===", retyped.len());
        println!("  A caller of the old type now sends what the other side does not expect.");
        for (entity, difference) in &retyped {
            println!(
                "  {}  {}: {} -> {}",
                entity.path(),
                difference.path,
                difference.repo.as_deref().unwrap_or_default(),
                difference.export.as_deref().unwrap_or_default()
            );
        }
    }
    if !report.retyped_arguments.is_empty() || !report.retyped_defaults.is_empty() {
        println!("\n=== retyped service parameters ===");
        for argument in &report.retyped_arguments {
            match (argument.mapped_source(), argument.unmappable_why()) {
                (Some(mapped), _) => println!(
                    "  {} line {}: {} {} -> {}",
                    argument.caller, argument.line, argument.parameter, argument.literal, mapped
                ),
                (_, Some(why)) => println!(
                    "  {} line {}: {} {}: cannot map to {} ({why})",
                    argument.caller,
                    argument.line,
                    argument.parameter,
                    argument.literal,
                    argument.to
                ),
                _ => println!(
                    "  {} line {}: {} {} accepted by {}",
                    argument.caller,
                    argument.line,
                    argument.parameter,
                    argument.literal,
                    argument.to
                ),
            }
        }
        for default in &report.retyped_defaults {
            println!(
                "  {} {}.{} default {:?}: cannot map to {} ({})",
                default.entity,
                default.service,
                default.parameter,
                default.default,
                default.to,
                default.why
            );
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

/// `twaco new building-block <name>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct NewBuildingBlock {
    #[arg(long = "type", value_name = "value")]
    pub(crate) kind: Option<String>,
    #[arg(long = "display-name", value_name = "text")]
    pub(crate) display_name: Option<String>,
    #[arg(long = "description", value_name = "text")]
    pub(crate) description: Option<String>,
    #[arg(long = "parent", value_name = "block")]
    pub(crate) parent: Option<String>,
    #[arg(long = "root", value_name = "dir")]
    pub(crate) root: Option<String>,
    #[arg(long = "base-extension", value_name = "value")]
    pub(crate) base_extension: Option<String>,
    #[arg(long = "model-logic")]
    pub(crate) model_logic: bool,
    #[arg(long = "no-management-shape")]
    pub(crate) no_management_shape: bool,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// Create a building block as files and register its project. Plans unless --apply.
pub(crate) fn new_building_block_cmd(solution: &Solution, args: &NewBuildingBlock) -> u8 {
    let [name] = args.operands.as_slice() else {
        eprintln!(
            "{} new building-block needs one <name>, such as Acme.Orders",
            style::prefix()
        );
        return FAILED;
    };
    let kind = match &args.kind {
        None => newblock::BlockType::Standard,
        Some(word) => match newblock::BlockType::from_word(word) {
            Some(kind) => kind,
            None => {
                eprintln!("{} --type is standard, abstract or implementation, not {word:?} (ui and test blocks are not created here yet)", style::prefix());
                return FAILED;
            }
        },
    };
    let request = newblock::Request {
        name: name.clone(),
        kind,
        display_name: args.display_name.clone(),
        description: args.description.clone().unwrap_or_default(),
        parent: args.parent.clone(),
        model_logic: args.model_logic,
        management_shape: !args.no_management_shape,
        root: args.root.clone(),
        base_extension: args.base_extension.clone(),
    };
    let apply = args.apply.apply;
    let request_name = request.name.clone();
    let request_kind = request.kind;
    let facade = commands::newblock::NewBlockRequest {
        request,
        mode: if apply { Mode::Apply } else { Mode::Plan },
        lock_label: "new building-block",
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::newblock::execute(solution, &facade, &mut notices) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    let plan = outcome.plan();
    let files: Vec<String> = plan
        .files
        .iter()
        .map(|file| solution.relative(&file.path))
        .collect();
    if args.json.json {
        let value = serde_json::json!({
            (if apply { "applied" } else { "plan" }): true,
            "name": request_name, "type": request_kind.word(), "root": plan.root,
            "files": files, "twaco_toml": plan.config_addition, "notes": plan.notes,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("new block plan serialises")
        );
        return OK;
    }
    println!(
        "new building-block {} ({}): {}",
        request_name,
        request_kind.word(),
        if apply {
            "created"
        } else {
            "a plan, nothing was written (pass --apply)"
        }
    );
    for file in &files {
        println!("  {file}");
    }
    println!(
        "  twaco.toml gets:{}",
        plan.config_addition.trim_end().replace('\n', "\n    ")
    );
    for note in &plan.notes {
        println!("  note: {note}");
    }
    if apply {
        println!("Next: twaco check; commit; twaco deploy --apply to create it on the server.");
    }
    OK
}

/// `twaco reproject <entity>... --to <project>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Reproject {
    #[arg(long = "to", value_name = "address")]
    pub(crate) to: Option<String>,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco reproject <entity>... --to <project>`: move entities to another project.
pub(crate) fn reproject_cmd(solution: &Solution, args: &Reproject) -> u8 {
    let Some(to) = args.to.as_ref() else {
        eprintln!("{} reproject needs --to <project>", style::prefix());
        return FAILED;
    };
    if args.operands.is_empty() {
        eprintln!("{} reproject needs at least one <entity>", style::prefix());
        return FAILED;
    }
    let apply = args.apply.apply;
    let request = commands::reproject::ReprojectRequest {
        request: twaco::core::reproject::Request {
            entities: args.operands.clone(),
            to: to.clone(),
        },
        mode: if apply { Mode::Apply } else { Mode::Plan },
        lock_label: "reproject",
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::reproject::execute(solution, &request, &mut notices) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    let plan = outcome.plan();
    if args.json.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&twaco::core::reproject::plan_json(solution, plan, apply))
                .expect("a plan serialises")
        );
        return OK;
    }
    let relative = |path: &std::path::Path| solution.relative(path);
    for item in &plan.moves {
        let verb = if apply { "moved" } else { "would move" };
        if item.new_path == item.old_path {
            println!(
                "{verb} {}/{} from {} to {}",
                item.collection, item.name, item.from, plan.to
            );
        } else {
            println!(
                "{verb} {}/{} from {} to {}: {} -> {}",
                item.collection,
                item.name,
                item.from,
                plan.to,
                relative(&item.old_path),
                relative(&item.new_path)
            );
        }
    }
    for warning in &plan.warnings {
        println!("{} {warning}", style::warn("deploy order:"));
    }
    if !apply {
        println!("dry run: nothing was written; pass --apply");
    }
    OK
}

/// `twaco retemplate <entity> [--to <template>] [--add-shapes <a,b>] [--remove-shapes <a,b>]`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Retemplate {
    #[arg(long = "to", value_name = "address")]
    pub(crate) to: Option<String>,
    #[arg(long = "add-shapes", value_name = "value")]
    pub(crate) add_shapes: Option<String>,
    #[arg(long = "remove-shapes", value_name = "value")]
    pub(crate) remove_shapes: Option<String>,
    #[arg(long = "accept-loss")]
    pub(crate) accept_loss: bool,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) detail: Detail,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// Change a template or implemented shapes. Plans unless --apply; a loss that holds data or is
/// still referenced is refused unless --accept-loss.
pub(crate) fn retemplate_cmd(solution: &Solution, args: &Retemplate) -> u8 {
    let [entity] = args.operands.as_slice() else {
        eprintln!("{} retemplate needs one <entity>", style::prefix());
        return FAILED;
    };
    let list = |value: &Option<String>| -> Vec<String> {
        value
            .as_ref()
            .map(|text| {
                text.split(',')
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    let request = retemplate::Request {
        entity: entity.clone(),
        template: args.to.clone(),
        add_shapes: list(&args.add_shapes),
        remove_shapes: list(&args.remove_shapes),
        accept_loss: args.accept_loss,
    };
    let apply = args.apply.apply;
    let facade = commands::retemplate::RetemplateRequest {
        request,
        mode: if apply { Mode::Apply } else { Mode::Plan },
        lock_label: "retemplate",
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::retemplate::execute(solution, &facade, &mut notices) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    let plan = outcome.plan();
    if args.json.json {
        let value = serde_json::json!({
            (if apply { "applied" } else { "plan" }): true,
            "entity": plan.request.entity, "collection": plan.collection, "file": plan.file_relative(solution),
            "affected": plan.affected, "gained": plan.gained, "lost": plan.lost,
            "needs_accept_loss": plan.blocked, "notes": plan.notes,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("retemplate plan serialises")
        );
        return OK;
    }
    println!(
        "retemplate {}: {}",
        plan.request.entity,
        if apply {
            "applied"
        } else {
            "a plan, nothing was written (pass --apply)"
        }
    );
    println!("  {}", plan.file_relative(solution));
    let detail = args.detail.detail;
    let shown = |items: &[retemplate::Change]| -> String {
        if items.is_empty() {
            return "nothing".to_string();
        }
        let count = |kind: &str| items.iter().filter(|change| change.kind == kind).count();
        let summary = format!(
            "{} (services {}, properties {}, configuration tables {})",
            items.len(),
            count("service"),
            count("property"),
            count("configuration table")
        );
        let limit = if detail {
            items.len()
        } else {
            items.len().min(6)
        };
        let names = items
            .iter()
            .take(limit)
            .map(|change| change.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "{summary}: {names}{}",
            if limit < items.len() { ", ..." } else { "" }
        )
    };
    println!(
        "  affects        {} entit{}",
        plan.affected.len(),
        if plan.affected.len() == 1 { "y" } else { "ies" }
    );
    println!("  gains          {}", shown(&plan.gained));
    println!("  loses          {}", shown(&plan.lost));
    for change in &plan.lost {
        if change.orphaned > 0 {
            println!(
                "  holds          {} {} is held by {} entit{}",
                change.kind,
                change.name,
                change.orphaned,
                if change.orphaned == 1 { "y" } else { "ies" }
            );
        }
        let limit = if detail {
            change.references.len()
        } else {
            change.references.len().min(3)
        };
        for line in change.references.iter().take(limit) {
            println!("  references     {} {}: {line}", change.kind, change.name);
        }
    }
    for note in &plan.notes {
        println!("  note: {note}");
    }
    if !plan.blocked.is_empty() && !apply {
        let shown_reasons = if detail {
            plan.blocked.len()
        } else {
            plan.blocked.len().min(3)
        };
        println!(
            "needs --accept-loss: {} reason(s): {}{}",
            plan.blocked.len(),
            plan.blocked[..shown_reasons].join("; "),
            if shown_reasons < plan.blocked.len() {
                "; ..."
            } else {
                ""
            }
        );
    }
    if apply {
        println!("Next: twaco check; commit; twaco deploy --apply.");
    }
    OK
}

/// `twaco move service|property <from> <to> <name>`.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum MoveMember {
    Service(MoveService),
    Property(Relocate),
}

/// `twaco copy service|property <from> <to> <name>`.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum CopyMember {
    Service(Relocate),
    Property(Relocate),
}

/// What every move and copy takes.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Relocate {
    #[arg(long = "as", value_name = "new")]
    pub(crate) new_name: Option<String>,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) detail: Detail,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco move service`, which can leave a delegate behind.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MoveService {
    #[command(flatten)]
    pub(crate) relocate: Relocate,
    #[arg(long = "leave-delegate")]
    pub(crate) leave_delegate: bool,
}

impl MoveMember {
    /// The command's [`USAGE`](super::spec::USAGE) path, which is also its lock label.
    pub(crate) fn path(&self) -> &'static str {
        match self {
            MoveMember::Service(_) => "move service",
            MoveMember::Property(_) => "move property",
        }
    }
}

impl CopyMember {
    /// The command's [`USAGE`](super::spec::USAGE) path, which is also its lock label.
    pub(crate) fn path(&self) -> &'static str {
        match self {
            CopyMember::Service(_) => "copy service",
            CopyMember::Property(_) => "copy property",
        }
    }
}

/// Move or copy a service or property between entities. Plans unless --apply; an apply checks the
/// sidecars still match the XML and exits 2 if they do not.
pub(crate) fn relocate_cmd(
    solution: &Solution,
    route: &str,
    args: &Relocate,
    leave_delegate: bool,
) -> u8 {
    let (verb, word) = route
        .split_once(' ')
        .expect("the route has a verb and a member");
    let member = relocate::Member::from_word(word).expect("the route names a member");
    let [from, to, name] = args.operands.as_slice() else {
        eprintln!("{} {route} needs <from> <to> <name>", style::prefix());
        return FAILED;
    };
    let request = relocate::Request {
        member,
        copy: verb == "copy",
        from: from.clone(),
        to: to.clone(),
        name: name.clone(),
        new_name: args.new_name.clone(),
        leave_delegate,
    };
    let apply = args.apply.apply;
    let facade = commands::relocate::RelocateRequest {
        request,
        mode: if apply { Mode::Apply } else { Mode::Plan },
        lock_label: route.to_string(),
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::relocate::execute(solution, &facade, &mut notices) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    let plan = outcome.plan();
    let problems = outcome.problems();
    if args.json.json {
        let value = serde_json::json!({
            (if apply { "applied" } else { "plan" }): true,
            "action": verb, "member": word, "from": plan.request.from, "to": plan.request.to,
            "name": plan.request.name, "as": plan.final_name,
            "files": plan.files(solution), "callers": plan.callers, "notes": plan.notes,
            "out_of_step": problems,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("relocation serialises")
        );
    } else {
        println!(
            "{verb} {word} {}.{} -> {}.{}: {}",
            plan.request.from,
            plan.request.name,
            plan.request.to,
            plan.final_name,
            if apply {
                "applied"
            } else {
                "a plan, nothing was written (pass --apply)"
            }
        );
        for file in plan.files(solution) {
            println!("  {file}");
        }
        for note in &plan.notes {
            println!("  note: {note}");
        }
        let shown = if args.detail.detail {
            plan.callers.first.len()
        } else {
            plan.callers.first.len().min(3)
        };
        for line in plan.callers.first.iter().take(shown) {
            println!("  caller: {line}");
        }
        for entity in problems {
            println!("  out of step: {entity} needs attention");
        }
        if apply {
            println!("Next: twaco check; commit; twaco deploy --apply. Property values stored on Things are not moved.");
        }
    }
    if problems.is_empty() {
        OK
    } else {
        FAILED
    }
}

fn display_relative(solution: &Solution, path: &Path) -> String {
    path.strip_prefix(&solution.root)
        .unwrap_or(path)
        .display()
        .to_string()
}

pub(crate) fn same_path(left: &Path, right: &Path) -> bool {
    fn comparable(path: &Path) -> String {
        let absolute = std::fs::canonicalize(path).unwrap_or_else(|_| {
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join(path)
            }
        });
        let text = absolute.to_string_lossy();
        if cfg!(windows) {
            text.to_ascii_lowercase()
        } else {
            text.into_owned()
        }
    }
    comparable(left) == comparable(right)
}
