use super::super::*;
use super::source::print_types_refresh;
use super::style;

/// Compare a designer's export with the solution: what it would revert, and what it would change.
///
/// Read-only. Summary by default; `--detail` lists every differing node, and
/// `--json` gives the same summary as JSON.
pub(crate) fn adopt_cmd(solution: &Solution, args: &Args) -> u8 {
    if args.names.len() != 1 {
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
    let takes = match parse_takes(args.values.get("--take").map(String::as_str)) {
        Ok(takes) => takes,
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    let request = commands::adopt::AdoptRequest {
        export: PathBuf::from(&args.names[0]),
        only: args.entity_filters.clone(),
        base: args.values.get("--base").cloned(),
        only_kind,
        takes,
        mode: if args.has("--apply") {
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

    if args.has("--json") {
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
            "identical": report.with_status(adopt::Status::Identical).count(),
            "base": report.base,
            "entities": report.entities.iter().map(|e| serde_json::json!({ "entity": e.entity.path(), "change": change_word(e.change), "kind": kind_word(e.kind) })).collect::<Vec<_>>(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&summary).expect("JSON values serialise")
        );
    } else {
        print_adopt_report(solution, &report, args.has("--detail"));
    }
    if let Some(outcome) = applied {
        println!("\n=== applied ({}) ===", outcome.lines.len());
        for line in &outcome.lines {
            println!("  {line}");
        }
        print_types_refresh(&outcome.types);
        let mashup_written = report.entities.iter().any(|entry| {
            entry.entity.collection == "Mashups"
                && outcome
                    .lines
                    .iter()
                    .any(|line| line == &format!("sidecar  {}", entry.entity.name))
        });
        if mashup_written {
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
    if args.has("--fail-on-revert") && reverts > 0 {
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

pub(crate) fn handoff_cmd(solution: &Solution, args: &Args) -> u8 {
    let Some(action) = args.names.first().map(String::as_str) else {
        eprintln!(
            "{} handoff needs `list` or `record <file.xml>... --name <name>`",
            style::prefix()
        );
        return FAILED;
    };
    let request = match action {
        "list" if args.names.len() == 1 => commands::handoff::HandoffRequest::List,
        "record" => {
            let Some(name) = args.values.get("--name") else {
                eprintln!("{} handoff record needs --name <name>", style::prefix());
                return FAILED;
            };
            let files = args.names[1..].iter().map(PathBuf::from).collect();
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
    let mode = if args.has("--apply") {
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

pub(crate) fn rename_cmd(solution: &Solution, route: &str, args: &Args) -> u8 {
    let word = route.strip_prefix("rename ").unwrap_or(route);
    let kind = rename::Kind::from_word(word).expect("the route names a rename kind");
    let mut request = match rename::Request::from_names(kind, &args.names) {
        Ok(request) => request,
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
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
    let request = commands::rename::RenameRequest {
        mode: if args.has("--apply") {
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
    if args.has("--json") {
        print_rename_json(solution, outcome, args.has("--text"));
    } else {
        print_rename_outcome(solution, outcome, args.has("--text"), args.has("--detail"));
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
        None => println!(
            "base: none (git history decides what is stale; record a handoff or pass --base)"
        ),
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

    let identical = report.with_status(adopt::Status::Identical).count();
    let ignored: usize = report.entities.iter().map(|e| e.ignored).sum();
    let volatile: usize = report.entities.iter().map(|e| e.volatile_ids).sum();
    println!("\n=== identical: {identical} entities ===");
    println!("    {volatile} regenerated binding/event ids and {ignored} configured-ignore node(s) were not counted as changes");
    if !detail && !changed.is_empty() {
        println!("run with --detail to see each differing node");
    }
}

/// Create a building block as files and register its project. Plans unless --apply.
pub(crate) fn new_building_block_cmd(solution: &Solution, args: &Args) -> u8 {
    let [name] = args.names.as_slice() else {
        eprintln!(
            "{} new building-block needs one <name>, such as Acme.Orders",
            style::prefix()
        );
        return FAILED;
    };
    let kind = match args.values.get("--type") {
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
        display_name: args.values.get("--display-name").cloned(),
        description: args
            .values
            .get("--description")
            .cloned()
            .unwrap_or_default(),
        parent: args.values.get("--parent").cloned(),
        model_logic: args.has("--model-logic"),
        management_shape: !args.has("--no-management-shape"),
        root: args.values.get("--root").cloned(),
        base_extension: args.values.get("--base-extension").cloned(),
    };
    let apply = args.has("--apply");
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
        .map(|file| {
            file.path
                .strip_prefix(&solution.root)
                .unwrap_or(&file.path)
                .display()
                .to_string()
                .replace('\\', "/")
        })
        .collect();
    if args.has("--json") {
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

/// Change a template or implemented shapes. Plans unless --apply; a loss that holds data or is
/// still referenced is refused unless --accept-loss.
pub(crate) fn retemplate_cmd(solution: &Solution, args: &Args) -> u8 {
    let [entity] = args.names.as_slice() else {
        eprintln!("{} retemplate needs one <entity>", style::prefix());
        return FAILED;
    };
    let list = |flag: &str| -> Vec<String> {
        args.values
            .get(flag)
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
        template: args.values.get("--to").cloned(),
        add_shapes: list("--add-shapes"),
        remove_shapes: list("--remove-shapes"),
        accept_loss: args.has("--accept-loss"),
    };
    let apply = args.has("--apply");
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
    if args.has("--json") {
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
    let detail = args.has("--detail");
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

/// Move or copy a service or property between entities. Plans unless --apply; an apply checks the
/// sidecars still match the XML and exits 2 if they do not.
pub(crate) fn relocate_cmd(solution: &Solution, route: &str, args: &Args) -> u8 {
    let (verb, word) = route
        .split_once(' ')
        .expect("the route has a verb and a member");
    let member = relocate::Member::from_word(word).expect("the route names a member");
    let [from, to, name] = args.names.as_slice() else {
        eprintln!("{} {route} needs <from> <to> <name>", style::prefix());
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
    if args.has("--json") {
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
        let shown = if args.has("--detail") {
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
