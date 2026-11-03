use super::flags::{Apply, Json};
use super::style;
use crate::{print_notices, DRIFT, FAILED, OK};
use twaco::core::commands::{self, lint_fix, Mode};
use twaco::core::config::Solution;
use twaco::core::lint_report::{self, LintRequest};

#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Lint {
    #[command(subcommand)]
    pub(crate) action: Option<LintAction>,
    #[arg(long = "entity", value_name = "name", conflicts_with = "explain")]
    pub(crate) entity: Vec<String>,
    #[arg(long = "service", value_name = "name", conflicts_with = "explain")]
    pub(crate) service: Option<String>,
    #[arg(long = "rule", value_name = "id", conflicts_with_all = ["explain", "print_config"])]
    pub(crate) rules: Vec<String>,
    #[arg(long = "explain", value_name = "id", conflicts_with = "print_config")]
    pub(crate) explain: Option<String>,
    #[arg(long = "print-config", conflicts_with = "explain")]
    pub(crate) print_config: bool,
    #[command(flatten)]
    pub(crate) json: Json,
}

#[derive(Debug, clap::Subcommand)]
pub(crate) enum LintAction {
    #[command(name = "fix-headers")]
    FixHeaders(FixHeaders),
}

/// `twaco lint fix-headers [--entity <name>] [--service <name>] [--apply] [--json]`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct FixHeaders {
    #[arg(long = "entity", value_name = "name")]
    pub(crate) entity: Vec<String>,
    #[arg(long = "service", value_name = "name")]
    pub(crate) service: Option<String>,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
}

pub(crate) fn lint(solution: &Solution, args: &Lint) -> u8 {
    if let Some(LintAction::FixHeaders(fix)) = &args.action {
        return fix_headers(solution, fix);
    }
    let entity = match args.entity.as_slice() {
        [] => None,
        [entity] => Some(entity.as_str()),
        _ => {
            eprintln!("lint: --entity takes one Collection/Name glob");
            return FAILED;
        }
    };
    if let Some(id) = &args.explain {
        return match lint_report::explain(solution, id) {
            Ok(explained) => {
                for rule in explained {
                    println!("{} ({}): {}", rule.id, rule.gate, rule.summary);
                    println!("severity here: {} (from {})", rule.severity, rule.origin);
                    if rule.advisory {
                        println!(
                            "`check` reports it without failing: [gates] advisory names {}",
                            rule.gate
                        );
                    }
                }
                OK
            }
            Err(error) => {
                eprintln!("lint: {error}");
                FAILED
            }
        };
    }
    if args.print_config {
        let config = lint_report::effective_config(solution, entity, args.service.as_deref());
        if args.json.json {
            println!(
                "{}",
                serde_json::to_string(&config).expect("config serialises")
            );
        } else {
            let width = config.iter().map(|rule| rule.rule.len()).max().unwrap_or(0);
            for rule in config {
                println!(
                    "{:<width$} = \"{}\"  # {}",
                    rule.rule, rule.severity, rule.origin
                );
            }
        }
        return OK;
    }
    let request = LintRequest {
        entity: entity.map(str::to_string),
        service: args.service.clone(),
        rules: args.rules.clone(),
    };
    let report = match lint_report::run(solution, &request) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("lint: {error}");
            return FAILED;
        }
    };
    let errors = report
        .findings
        .iter()
        .filter(|finding| finding.severity == "error")
        .count();
    let warnings = report.findings.len() - errors;
    for finding in &report.findings {
        if args.json.json {
            println!(
                "{}",
                serde_json::to_string(finding).expect("finding serialises")
            );
        } else if finding.line == 0 {
            println!(
                "{} [{}] {}: {}",
                finding.file, finding.rule, finding.severity, finding.message
            );
        } else {
            println!(
                "{}:{} [{}] {}: {}",
                finding.file, finding.line, finding.rule, finding.severity, finding.message
            );
        }
    }
    if !args.json.json {
        if report.findings.is_empty() {
            println!("no findings in {} script(s)", report.examined);
        } else {
            println!(
                "{errors} error(s), {warnings} warning(s) in {} script(s)",
                report.examined
            );
        }
    }
    if errors > 0 || !report.broken.is_empty() {
        DRIFT
    } else {
        OK
    }
}

fn fix_headers(solution: &Solution, args: &FixHeaders) -> u8 {
    let entity = match args.entity.as_slice() {
        [] => None,
        [entity] => Some(entity.clone()),
        _ => {
            eprintln!("lint fix-headers: --entity takes one Collection/Name glob");
            return FAILED;
        }
    };
    let apply = args.apply.apply;
    let request = lint_fix::FixHeadersRequest {
        mode: if apply { Mode::Apply } else { Mode::Plan },
        entity,
        service: args.service.clone(),
        lock_label: "lint fix-headers",
    };
    let mut notices = commands::Notices::default();
    let result = lint_fix::execute(solution, &request, &mut notices);
    print_notices(&notices);
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    if args.json.json {
        println!(
            "{}",
            serde_json::json!({
                "apply": apply,
                "examined": outcome.examined,
                "changed": outcome.changed(),
                "scripts": outcome.scripts,
                "failed": outcome.failed,
            })
        );
    } else {
        for script in &outcome.scripts {
            if !script.changes.is_empty() {
                let verb = if apply { "fixed" } else { "would fix" };
                println!("{verb} {}: {}", script.file, script.changes.join("; "));
            }
            for why in &script.not_fixable {
                println!("  {}: not fixed here, {why}", script.file);
            }
        }
        for why in &outcome.failed {
            eprintln!("{} {why}", style::prefix());
        }
        let changed = outcome.changed();
        match (changed, apply) {
            (0, _) => println!("{} header(s) match their definitions", outcome.examined),
            (_, false) => println!(
                "{changed} of {} header(s) to fix; --apply writes them",
                outcome.examined
            ),
            (_, true) => println!(
                "fixed {changed} of {} header(s); `twaco sync` writes them into the entity XML",
                outcome.examined
            ),
        }
    }
    if !outcome.failed.is_empty() {
        FAILED
    } else if !apply && outcome.changed() > 0 {
        DRIFT
    } else {
        OK
    }
}
