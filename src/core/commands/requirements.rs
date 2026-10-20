//! Shared locking, plan/apply and profile policy for the requirements contract.

use super::{lock_workspace, Mode, Notices};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::progress::Progress;
use crate::core::{lock, profile, requirements, transaction};

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("{0}")]
    Requirements(#[from] requirements::Error),
    #[error("{0}")]
    Lock(#[from] lock::LockError),
    #[error("{0}")]
    Write(#[from] transaction::TransactionError),
    #[error("{0}")]
    Profile(#[from] profile::ProfileError),
}

impl Coded for CommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Requirements(error) => error.code(),
            Self::Lock(error) => error.code(),
            Self::Write(error) => error.code(),
            Self::Profile(error) => error.code(),
        }
    }
}

pub struct InitRequest {
    pub mode: Mode,
    pub lock_label: &'static str,
}

pub struct InitOutcome {
    pub text: String,
    pub written: bool,
    pub report: requirements::Report,
}

/// Draft offline and create a new file only. Apply discovers and refuses an existing file
/// under the same workspace lock as the write; the transaction also enforces creation only.
pub fn execute_init(
    solution: &Solution,
    request: &InitRequest,
    notices: &mut Notices,
) -> Result<InitOutcome, CommandError> {
    let lock = match request.mode {
        Mode::Apply => Some(lock_workspace(solution, request.lock_label, notices)?),
        Mode::Plan => None,
    };
    let path = solution.root.join(requirements::FILE);
    if path.symlink_metadata().is_ok() {
        return Err(requirements::Error(format!(
            "{} already exists; run twaco requirements check to see what changed",
            path.display()
        ))
        .into());
    }
    let collected = requirements::collect(solution)?;
    let text = requirements::draft(&collected);
    let contract = requirements::parse(&text)?;
    let report = requirements::offline(&contract, &collected);
    if let Some(lock) = &lock {
        let mut operation = transaction::Transaction::new(&solution.root, request.lock_label);
        operation.create_file(&path, text.clone().into_bytes())?;
        operation.apply(lock)?;
    }
    Ok(InitOutcome {
        text,
        written: lock.is_some(),
        report,
    })
}

pub struct CheckRequest {
    /// No profile is loaded unless a server read was explicitly requested.
    pub server: Option<String>,
}

pub fn execute_check<R, F>(
    solution: &Solution,
    request: &CheckRequest,
    open: F,
    progress: &dyn Progress,
) -> Result<requirements::Report, CommandError>
where
    R: requirements::Remote,
    F: FnOnce(profile::Profile) -> R,
{
    let contract = requirements::load(solution)?;
    let collected = requirements::collect(solution)?;
    let mut report = requirements::offline(&contract, &collected);
    if let Some(name) = &request.server {
        let remote = open(profile::load(&solution.root, name)?);
        requirements::server_check(&contract, &collected, &remote, &mut report, progress);
    }
    Ok(report)
}
