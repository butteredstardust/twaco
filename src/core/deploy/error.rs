use super::super::baseline::BaselineError;
use super::super::server::ServerError;
use super::{EntityPlan, ParseFailure, Report};
use std::fmt;

#[derive(Debug)]
pub enum DeployError {
    Baseline(BaselineError),
    Working {
        collection: String,
        name: String,
        why: String,
    },
    Server {
        collection: String,
        name: String,
        why: String,
    },
    ParseUnavailable {
        entity: String,
        service: String,
        source: ServerError,
    },
    ParseFailed(Vec<ParseFailure>),
    Conflicts(Vec<EntityPlan>),
    /// `imported` are the projects that imported before this one; the baseline records them.
    Import {
        project: String,
        source: ServerError,
        imported: Vec<String>,
    },
    UnknownPlaceholder {
        project: String,
        key: String,
    },
    /// Every project had imported, and the baseline records them, when a call failed.
    Call {
        project: String,
        target: String,
        service: String,
        why: String,
        imported: Vec<String>,
    },
    NotKept(Box<Report>),
    /// A failure after projects had imported, which the server therefore already holds.
    AfterImport {
        imported: Vec<String>,
        source: Box<DeployError>,
    },
    /// A failure, and then the baseline for what did import could not be written either.
    Unrecorded {
        failure: Box<DeployError>,
        why: BaselineError,
        imported: Vec<String>,
    },
}

impl fmt::Display for DeployError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeployError::Baseline(error) => write!(f, "{error}"),
            DeployError::Working { collection, name, why } => {
                write!(f, "{collection}/{name}: the bundled entity cannot be hashed: {why}")
            }
            DeployError::Server { collection, name, why } => {
                write!(f, "{collection}/{name}: the server copy cannot be checked: {why}")
            }
            DeployError::ParseUnavailable { entity, service, source } => {
                write!(f, "{entity}/{service}: live parse could not run: {source}")
            }
            DeployError::ParseFailed(failures) => {
                write!(f, "{} service script(s) failed live parse", failures.len())
            }
            DeployError::Conflicts(conflicts) => {
                write!(f, "{} entity conflict(s) refuse this deploy", conflicts.len())
            }
            DeployError::Import { project, source, imported } => {
                write!(f, "project {project} import failed: {source}")?;
                if !imported.is_empty() {
                    write!(f, "; already imported and recorded: {}", imported.join(", "))?;
                }
                Ok(())
            }
            DeployError::UnknownPlaceholder { project, key } => write!(
                f,
                "project {project} uses unknown profile key {key:?} in ${{profile:{key}}}"
            ),
            DeployError::Call { project, target, service, why, imported } => {
                write!(f, "project {project} call {target}.{service} failed: {why}")?;
                if !imported.is_empty() {
                    write!(f, "; already imported and recorded: {}", imported.join(", "))?;
                }
                Ok(())
            }
            DeployError::AfterImport { imported, source } => {
                write!(f, "{source}; already imported and recorded: {}", imported.join(", "))
            }
            DeployError::Unrecorded { failure, why, imported } => write!(
                f,
                "{failure}; and the baseline for what had imported ({}) could not be written: {why}",
                if imported.is_empty() { "nothing".to_string() } else { imported.join(", ") }
            ),
            DeployError::NotKept(report) => {
                write!(f, "{} imported entity/entities were not kept as sent", report.not_kept.len())
            }
        }
    }
}

impl std::error::Error for DeployError {}
