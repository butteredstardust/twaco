use super::super::config::Solution;
use super::write::write;
use super::TypesError;
use serde_json::json;
use std::time::Duration;

#[derive(Debug, Default)]
pub struct Outcome {
    pub entities: usize,
    pub data_shapes: usize,
    pub services: usize,
    pub files_written: usize,
    pub skipped: Vec<String>,
    pub gitignore_covers_types: bool,
}

#[derive(Debug, Default)]
pub struct Refresh {
    pub files_written: Option<usize>,
    pub warning: Option<String>,
}

/// Keep an opted-in solution's declarations current after another command wrote source data.
/// Failure is advisory: the command's own writes have already succeeded and remain successful.
pub fn refresh_after_write(solution: &Solution, wrote: bool) -> Refresh {
    if !wrote || !solution.root.join(".twaco/types").exists() {
        return Refresh::default();
    }
    match write(solution) {
        Ok(outcome) => Refresh {
            files_written: Some(outcome.files_written),
            warning: None,
        },
        Err(error) => Refresh {
            files_written: None,
            warning: Some(error.to_string()),
        },
    }
}

#[derive(Debug)]
pub struct PlatformOutcome {
    pub templates: usize,
    pub shapes: usize,
    pub resources: usize,
    pub skipped: Vec<String>,
    pub types: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeFinding {
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub code: String,
    pub message: String,
}

#[derive(Debug)]
pub struct CheckOutcome {
    pub declarations: Outcome,
    pub findings: Vec<TypeFinding>,
    pub affected_services: usize,
    pub services: usize,
    pub elapsed: Duration,
}

/// One finding in the JSON Lines protocol consumed by a declared `[[check]]` hook.
pub fn finding_json(finding: &TypeFinding) -> String {
    json!({
        "schema": 1,
        "gate": "types",
        "file": finding.file,
        "line": finding.line,
        "rule": format!("TS{}", finding.code),
        "message": format!("{} (column {})", finding.message, finding.column),
    })
    .to_string()
}

pub fn check_summary(outcome: &CheckOutcome) -> String {
    format!(
        "types: {} finding(s) in {} of {} services (tsc in {:.1} s)",
        outcome.findings.len(),
        outcome.affected_services,
        outcome.services,
        outcome.elapsed.as_secs_f64()
    )
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct CheckError(pub(crate) String);

impl From<TypesError> for CheckError {
    fn from(value: TypesError) -> Self {
        Self(value.to_string())
    }
}
