use super::flags::Json;
use crate::{DRIFT, FAILED, OK};
use twaco::core::config::Solution;
use twaco::core::lint_report::{self, LintRequest};

#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Lint {
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

pub(crate) fn lint(solution: &Solution, args: &Lint) -> u8 {
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
