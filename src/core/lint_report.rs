//! A focused, read-only view of the configurable script gates.

use super::check::{self, GateResult};
use super::config::{Severity, Solution};
use super::{glob, rules, workspace};
use std::collections::BTreeSet;
use std::path::Path;

#[derive(Debug, Default, Clone)]
pub struct LintRequest {
    pub entity: Option<String>,
    pub service: Option<String>,
    pub rules: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LintFinding {
    pub gate: String,
    pub file: String,
    pub line: usize,
    pub rule: String,
    pub severity: String,
    pub message: String,
}

#[derive(Debug, Default)]
pub struct LintReport {
    pub examined: usize,
    pub findings: Vec<LintFinding>,
    pub broken: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Explained {
    pub id: String,
    pub gate: String,
    pub summary: String,
    pub severity: String,
    pub origin: String,
    /// `[gates] advisory` names the rule's gate, so `check` reports it without failing.
    pub advisory: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Effective {
    pub rule: String,
    pub gate: String,
    pub severity: String,
    pub origin: String,
}

/// Runs the script rules only. `[gates].advisory` never changes lint severities.
pub fn run(solution: &Solution, request: &LintRequest) -> Result<LintReport, String> {
    let selected = selected_rules(&request.rules)?;
    let discovery = workspace::discover(solution);
    let lint = solution.effective_lint();
    let gates = [
        check::formatting_with(solution, &discovery, &lint),
        check::script_traps_with(solution, &discovery, &lint),
        check::code_order_with(solution, &discovery, &lint),
    ];
    let scripts = script_addresses(solution, &discovery, request);
    let mut report = LintReport {
        examined: scripts.len(),
        ..LintReport::default()
    };
    let addresses: BTreeSet<String> = scripts.into_iter().collect();
    for gate in &gates {
        collect_gate(gate, &addresses, &selected, &mut report);
    }
    report.findings.sort_by(|left, right| {
        (&left.file, left.line, &left.rule).cmp(&(&right.file, right.line, &right.rule))
    });
    Ok(report)
}

pub fn explain(solution: &Solution, id_or_group: &str) -> Result<Vec<Explained>, String> {
    let selected = selected_rules(&[id_or_group.to_string()])?;
    let lint = solution.effective_lint();
    Ok(rules::RULES
        .iter()
        .filter(|rule| selected.contains(&rule.id))
        .map(|rule| {
            let (severity, origin) = lint.severity_with_origin(rule.id, None, "");
            Explained {
                id: rule.id.to_string(),
                gate: rule.gate.to_string(),
                summary: rule.summary.to_string(),
                severity: severity_name(severity).to_string(),
                origin,
                advisory: solution.gates.advisory.iter().any(|gate| gate == rule.gate),
            }
        })
        .collect())
}

pub fn effective_config(
    solution: &Solution,
    entity: Option<&str>,
    service: Option<&str>,
) -> Vec<Effective> {
    let lint = solution.effective_lint();
    rules::RULES
        .iter()
        .map(|rule| {
            let (severity, origin) =
                lint.severity_with_origin(rule.id, entity, service.unwrap_or(""));
            Effective {
                rule: rule.id.to_string(),
                gate: rule.gate.to_string(),
                severity: severity_name(severity).to_string(),
                origin,
            }
        })
        .collect()
}

fn selected_rules(entries: &[String]) -> Result<Vec<&'static str>, String> {
    if entries.is_empty() {
        return Ok(rules::RULES.iter().map(|rule| rule.id).collect());
    }
    let mut selected = Vec::new();
    for entry in entries {
        let matching: Vec<_> = rules::RULES
            .iter()
            .filter(|rule| rule.id == entry || rule.id.starts_with(&format!("{entry}.")))
            .map(|rule| rule.id)
            .collect();
        if matching.is_empty() {
            return Err(format!("unknown lint rule or group {entry:?}"));
        }
        for id in matching {
            if !selected.contains(&id) {
                selected.push(id);
            }
        }
    }
    Ok(selected)
}

/// The scripts the request covers, as the gates name them: every sidecar script `check` reads,
/// so a script whose entity file is missing is linted too unless an entity filter is given.
fn script_addresses(
    solution: &Solution,
    discovery: &workspace::Discovery,
    request: &LintRequest,
) -> Vec<String> {
    workspace::script_files(solution)
        .files
        .into_iter()
        .filter(|path| {
            let entity = check::script_entity(solution, discovery, path)
                .map(|(collection, name)| format!("{collection}/{name}"));
            let service = path
                .parent()
                .and_then(Path::file_name)
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            request.entity.as_deref().is_none_or(|pattern| {
                entity
                    .as_deref()
                    .is_some_and(|entity| glob::matches(pattern, entity))
            }) && request
                .service
                .as_deref()
                .is_none_or(|pattern| glob::matches(pattern, &service))
        })
        .map(|path| check::relative(solution, &path))
        .collect()
}

fn collect_gate(
    gate: &GateResult,
    scripts: &BTreeSet<String>,
    selected: &[&str],
    report: &mut LintReport,
) {
    if let Some(reason) = &gate.broken {
        report.broken.push(format!("{}: {reason}", gate.name));
    }
    for (finding, severity) in gate
        .findings
        .iter()
        .map(|finding| (finding, Severity::Error))
        .chain(
            gate.warnings
                .iter()
                .map(|finding| (finding, Severity::Warn)),
        )
    {
        let Some(rule) = rules::RULES
            .iter()
            .find(|rule| rule.gate == finding.gate && rule.rule == finding.rule)
        else {
            if scripts.contains(&finding.file) {
                report
                    .broken
                    .push(format!("{}: {}", gate.name, finding.message));
            }
            continue;
        };
        if scripts.contains(&finding.file) && selected.contains(&rule.id) {
            report.findings.push(LintFinding {
                gate: finding.gate.clone(),
                file: finding.file.clone(),
                line: finding.line,
                rule: rule.id.to_string(),
                severity: severity_name(severity).to_string(),
                message: finding.message.clone(),
            });
        }
    }
}

fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warn => "warn",
        Severity::Off => "off",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_ids_and_groups_select_their_registered_rules() {
        assert_eq!(selected_rules(&["format".to_string()]).unwrap().len(), 2);
        assert_eq!(
            selected_rules(&["traps.for-in".to_string()]).unwrap(),
            vec!["traps.for-in"]
        );
        assert!(selected_rules(&["unknown".to_string()])
            .unwrap_err()
            .contains("unknown"));
    }

    /// Two entities with one script each, and a sidecar whose entity file is missing.
    fn fixture(config: &str) -> (tempfile::TempDir, Solution) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join("twaco.toml"),
            format!(
                "{config}
[[project]]
name = \"P\"
"
            ),
        )
        .unwrap();
        std::fs::create_dir_all(root.join("Things")).unwrap();
        for (name, service, code) in [
            (
                "P.A",
                "Run",
                "while (more()) {
    step();
}
for (var k in m) {
    use(k);
}",
            ),
            (
                "P.B",
                "Import",
                "for (var k in m) {
    use(k);
}",
            ),
        ] {
            std::fs::write(
                root.join(format!("Things/{name}.xml")),
                format!("<Entities><Things><Thing name=\"{name}\" projectName=\"P\"></Thing></Things></Entities>"),
            )
            .unwrap();
            let folder = root.join(format!("src/{name}/services/{service}"));
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(folder.join("script.js"), code).unwrap();
        }
        let orphan = root.join("src/P.Gone/services/Lost");
        std::fs::create_dir_all(&orphan).unwrap();
        std::fs::write(
            orphan.join("script.js"),
            "for (var k in m) {
    use(k);
}",
        )
        .unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (dir, solution)
    }

    fn found(solution: &Solution, request: LintRequest) -> (usize, Vec<String>) {
        let report = run(solution, &request).unwrap();
        let findings = report
            .findings
            .iter()
            .map(|f| format!("{}:{} {} {}", f.file, f.line, f.rule, f.severity))
            .collect();
        (report.examined, findings)
    }

    fn traps() -> LintRequest {
        LintRequest {
            rules: vec!["traps".to_string()],
            ..LintRequest::default()
        }
    }

    #[test]
    fn every_script_check_reads_is_linted_in_order() {
        let (_dir, solution) = fixture("");
        assert_eq!(
            found(&solution, traps()),
            (
                3,
                vec![
                    "src/P.A/services/Run/script.js:1 traps.unbounded-loop error".to_string(),
                    "src/P.A/services/Run/script.js:4 traps.for-in error".to_string(),
                    "src/P.B/services/Import/script.js:1 traps.for-in error".to_string(),
                    "src/P.Gone/services/Lost/script.js:1 traps.for-in error".to_string(),
                ]
            )
        );
    }

    #[test]
    fn filters_narrow_scripts_and_rules() {
        let (_dir, solution) = fixture("");
        let by = |entity: Option<&str>, service: Option<&str>, rules: &[&str]| {
            found(
                &solution,
                LintRequest {
                    entity: entity.map(str::to_string),
                    service: service.map(str::to_string),
                    rules: rules.iter().map(|r| r.to_string()).collect(),
                },
            )
        };
        assert_eq!(by(Some("Things/P.A"), None, &["traps"]).1.len(), 2);
        assert_eq!(
            by(Some("ThingTemplates/P.A"), None, &["traps"]),
            (0, vec![]),
            "another collection"
        );
        assert_eq!(by(Some("**"), None, &["traps"]).0, 2, "no entity, no match");
        assert_eq!(by(None, Some("Imp*"), &["traps"]).1.len(), 1);
        assert_eq!(by(None, None, &["traps.unbounded-loop"]).1.len(), 1);
        assert!(by(None, None, &["order"]).1.is_empty());
        assert!(run(
            &solution,
            &LintRequest {
                rules: vec!["traps.nope".to_string()],
                ..LintRequest::default()
            }
        )
        .unwrap_err()
        .contains("traps.nope"));
    }

    #[test]
    fn severities_come_from_lint_and_not_from_advisory_gates() {
        let (_dir, solution) = fixture(
            "[gates]
advisory = [\"script traps\"]
[lint.rules.\"traps.for-in\"]
severity = \"warn\"
[[lint.override]]
entity = \"Things/P.B\"
[lint.override.rules.\"traps.for-in\"]
severity = \"off\"
",
        );
        assert_eq!(
            found(&solution, traps()).1,
            vec![
                "src/P.A/services/Run/script.js:1 traps.unbounded-loop error".to_string(),
                "src/P.A/services/Run/script.js:4 traps.for-in warn".to_string(),
                "src/P.Gone/services/Lost/script.js:1 traps.for-in warn".to_string(),
            ],
            "advisory leaves error as error; off drops P.B"
        );
    }
}
