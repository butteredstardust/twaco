//! Stable categories for errors exposed by adapters.

use serde::Serialize;

/// A machine-readable category for a failure and the caller's next action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Correct the supplied arguments and try again.
    InvalidArguments,
    /// Choose an entity that exists in the solution.
    UnknownEntity,
    /// Disambiguate the requested entity.
    Ambiguous,
    /// Choose a name or destination that does not already exist.
    AlreadyExists,
    /// Ask a person to review or explicitly allow the refused operation.
    GuardRefused,
    /// Inspect and fix the failed gate before trying again.
    GateFailed,
    /// Re-plan because the repository changed since planning.
    StalePlan,
    /// Retry later, after the other workspace writer finishes.
    WorkspaceLocked,
    /// Re-plan against the server state before trying again.
    ServerConflict,
    /// Retry later or inspect the server connection.
    ServerUnreachable,
    /// Inspect the server response and configuration.
    ServerError,
    /// Inspect the server state before retrying an operation that was not verified.
    NotVerified,
    /// Ask a person to repair the incomplete rollback.
    RollbackFailed,
    /// Inspect the repository and local filesystem, then retry.
    IoError,
    /// Inspect and correct invalid repository or server data.
    InvalidData,
    /// Inspect the message; this failure has not yet been classified.
    Unclassified,
}

impl ErrorCode {
    pub const ALL: [Self; 16] = [
        Self::InvalidArguments,
        Self::UnknownEntity,
        Self::Ambiguous,
        Self::AlreadyExists,
        Self::GuardRefused,
        Self::GateFailed,
        Self::StalePlan,
        Self::WorkspaceLocked,
        Self::ServerConflict,
        Self::ServerUnreachable,
        Self::ServerError,
        Self::NotVerified,
        Self::RollbackFailed,
        Self::IoError,
        Self::InvalidData,
        Self::Unclassified,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidArguments => "invalid_arguments",
            Self::UnknownEntity => "unknown_entity",
            Self::Ambiguous => "ambiguous",
            Self::AlreadyExists => "already_exists",
            Self::GuardRefused => "guard_refused",
            Self::GateFailed => "gate_failed",
            Self::StalePlan => "stale_plan",
            Self::WorkspaceLocked => "workspace_locked",
            Self::ServerConflict => "server_conflict",
            Self::ServerUnreachable => "server_unreachable",
            Self::ServerError => "server_error",
            Self::NotVerified => "not_verified",
            Self::RollbackFailed => "rollback_failed",
            Self::IoError => "io_error",
            Self::InvalidData => "invalid_data",
            Self::Unclassified => "unclassified",
        }
    }
}

/// An error whose stable category can be returned by an adapter.
pub trait Coded {
    fn code(&self) -> ErrorCode;
}

impl Coded for super::lock::LockError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Held { .. } => ErrorCode::WorkspaceLocked,
            Self::Io { .. } => ErrorCode::IoError,
            Self::Recovery { .. } => ErrorCode::RollbackFailed,
        }
    }
}
impl Coded for super::transaction::TransactionError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Invalid(_) => ErrorCode::InvalidArguments,
            Self::Stale(_) => ErrorCode::StalePlan,
            Self::Io { .. }
            | Self::Failed {
                rolled_back: true, ..
            } => ErrorCode::IoError,
            Self::Failed {
                rolled_back: false, ..
            } => ErrorCode::RollbackFailed,
        }
    }
}
impl Coded for super::server::ServerError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::InvalidUrl(_) => ErrorCode::InvalidArguments,
            Self::Transport { .. } => ErrorCode::ServerUnreachable,
            Self::Http { .. }
            | Self::Rejected { .. }
            | Self::UnsupportedCharset(_)
            | Self::InvalidUtf8 { .. }
            | Self::InvalidResponse { .. } => ErrorCode::ServerError,
        }
    }
}
impl Coded for super::workspace::WorkspaceError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::UnknownEntity { .. } => ErrorCode::UnknownEntity,
            Self::Ambiguous { .. } => ErrorCode::Ambiguous,
            Self::InvalidCallTarget { .. } => ErrorCode::InvalidArguments,
            Self::Io { .. } => ErrorCode::IoError,
        }
    }
}
impl Coded for super::config::ConfigError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::NotFound { .. }
            | Self::NoProjects { .. }
            | Self::DuplicateProject { .. }
            | Self::BlankProjectName
            | Self::BlankCheckName
            | Self::EmptyCheckCommand { .. }
            | Self::EmptyTypesCompiler
            | Self::EscapingPath { .. }
            | Self::UnknownDependency { .. }
            | Self::DependencyCycle { .. }
            | Self::Invalid { .. } => ErrorCode::InvalidData,
            Self::Unreadable { .. } => ErrorCode::IoError,
        }
    }
}
impl Coded for super::profile::ProfileError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::InvalidName(_) => ErrorCode::InvalidArguments,
            Self::Missing { .. }
            | Self::Incomplete { .. }
            | Self::Unreadable { .. }
            | Self::Invalid { .. } => ErrorCode::InvalidData,
        }
    }
}
impl Coded for super::rename::RenameError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::InvalidNew { .. }
            | Self::InvalidOld { .. }
            | Self::Same { .. }
            | Self::Nested { .. } => ErrorCode::InvalidArguments,
            Self::Unknown { .. } => ErrorCode::UnknownEntity,
            Self::Ambiguous { .. } => ErrorCode::Ambiguous,
            Self::Exists { .. } => ErrorCode::AlreadyExists,
            Self::DatabaseHalf { .. }
            | Self::ServiceScope { .. }
            | Self::TableScope { .. }
            | Self::WouldDesync { .. } => ErrorCode::GuardRefused,
            Self::GatesFail { .. } => ErrorCode::GateFailed,
            Self::Stale { .. } | Self::PlanChanged { .. } => ErrorCode::StalePlan,
            Self::RollbackFailed { .. } => ErrorCode::RollbackFailed,
            Self::Io { .. } | Self::Apply { .. } => ErrorCode::IoError,
            Self::Xml { .. }
            | Self::Splice { .. }
            | Self::InvalidLedger { .. }
            | Self::Unreadable { .. } => ErrorCode::InvalidData,
        }
    }
}
impl Coded for super::push::Refusal {
    fn code(&self) -> ErrorCode {
        match self {
            Self::DeletedOnServer { .. } | Self::UnknownAncestor { .. } | Self::Conflict { .. } => {
                ErrorCode::ServerConflict
            }
        }
    }
}
impl Coded for super::push::PushError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Working(_) | Self::Server(_) => ErrorCode::InvalidData,
            Self::Remote(e) => e.code(),
            Self::Unverified(_) | Self::NotKept { .. } => ErrorCode::NotVerified,
            Self::Baseline(e) => e.code(),
        }
    }
}
impl Coded for super::baseline::BaselineError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Io { .. } => ErrorCode::IoError,
            Self::Invalid { .. } => ErrorCode::InvalidData,
            Self::Missing { .. } => ErrorCode::InvalidData,
        }
    }
}
impl Coded for super::entity_delete::DeleteError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Remote { why, .. } => why.code(),
            Self::Target(_) => ErrorCode::InvalidArguments,
            Self::Ledger { .. } => ErrorCode::InvalidData,
            Self::Write { .. } | Self::Backup(_) => ErrorCode::IoError,
        }
    }
}
impl Coded for super::entity_carry::CarryError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Ledger { .. } => ErrorCode::InvalidData,
            Self::Arguments { .. } => ErrorCode::InvalidArguments,
        }
    }
}
impl Coded for super::export::ExportError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Remote(e) => e.code(),
            Self::Invalid(_) => ErrorCode::InvalidArguments,
        }
    }
}
impl Coded for super::backup::BackupError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Remote { why, .. } => why.code(),
            Self::Io { .. } => ErrorCode::IoError,
            Self::Unreadable { .. } | Self::Invalid { .. } => ErrorCode::InvalidData,
            Self::NoSuchSet { .. } => ErrorCode::UnknownEntity,
            Self::Plan(_) => ErrorCode::GuardRefused,
        }
    }
}
impl Coded for super::relocate::RelocateError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Invalid(_) => ErrorCode::InvalidArguments,
            Self::Unknown { .. } | Self::NotDeclared { .. } => ErrorCode::UnknownEntity,
            Self::Unreadable { .. } | Self::Xml { .. } => ErrorCode::InvalidData,
            Self::Inherited { .. } | Self::Refused(_) => ErrorCode::GuardRefused,
            Self::Exists { .. } => ErrorCode::AlreadyExists,
            Self::Apply { .. } => ErrorCode::IoError,
            Self::Verification(_) => ErrorCode::StalePlan,
            Self::Write(error) => error.code(),
        }
    }
}
impl Coded for super::newblock::NewBlockError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Invalid(_) => ErrorCode::InvalidArguments,
            Self::Exists(_) => ErrorCode::AlreadyExists,
            Self::Io { .. } => ErrorCode::IoError,
            Self::Write(error) => error.code(),
        }
    }
}
impl Coded for super::entity_key::KeyError {
    fn code(&self) -> ErrorCode {
        ErrorCode::InvalidArguments
    }
}
impl Coded for super::types::TypesError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Workspace(error) => error.code(),
            Self::Remote(error) => error.code(),
            Self::Platform(_) => ErrorCode::InvalidData,
        }
    }
}
impl Coded for super::retemplate::RetemplateError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Invalid(_) => ErrorCode::InvalidArguments,
            Self::Unknown { .. } => ErrorCode::UnknownEntity,
            Self::Unreadable { .. } | Self::Xml { .. } => ErrorCode::InvalidData,
            Self::Loss { .. } => ErrorCode::GuardRefused,
            Self::Apply { .. } => ErrorCode::IoError,
        }
    }
}
impl Coded for super::datatable_copy::CopyError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Arguments(_) => ErrorCode::InvalidArguments,
            Self::Ledger(_) => ErrorCode::InvalidData,
            Self::Refused(_) => ErrorCode::GuardRefused,
            Self::Server(error) => error.code(),
        }
    }
}
impl Coded for super::catalog::CatalogError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Invalid(_) => ErrorCode::InvalidArguments,
            Self::UnknownEntity(_) => ErrorCode::UnknownEntity,
            Self::Ambiguous(_) => ErrorCode::Ambiguous,
        }
    }
}
impl Coded for super::impact::ImpactError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Entity(error) => error.code(),
        }
    }
}
impl Coded for super::config_table::TableError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Remote(error) | Self::PartlyRestored { why: error, .. } => error.code(),
            Self::Shape(_) | Self::Backup { .. } | Self::Repository(_) => ErrorCode::InvalidData,
            Self::WrongBackup { .. }
            | Self::NoPrimaryKey
            | Self::KeyMismatch { .. }
            | Self::BadKey(_) => ErrorCode::GuardRefused,
            Self::NotRestored(_) => ErrorCode::NotVerified,
        }
    }
}
impl Coded for super::repo::RepoError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Remote(error) => error.code(),
            Self::Shape(_) => ErrorCode::InvalidData,
            Self::Invalid(_) => ErrorCode::InvalidArguments,
            Self::Local { .. } => ErrorCode::IoError,
        }
    }
}
impl Coded for super::settings::SettingsError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Remote(error) => error.code(),
            Self::Shape(_) => ErrorCode::InvalidData,
            Self::Invalid(_) => ErrorCode::InvalidArguments,
        }
    }
}
impl Coded for super::extensions::ExtensionError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Remote(error) => error.code(),
            Self::Shape(_) => ErrorCode::InvalidData,
            Self::Invalid(_) => ErrorCode::InvalidArguments,
        }
    }
}
impl Coded for super::logs::LogsError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Remote(error) => error.code(),
            Self::Shape(_) => ErrorCode::InvalidData,
            Self::Invalid(_) => ErrorCode::InvalidArguments,
        }
    }
}
impl Coded for super::package::PackageError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Invalid(_) => ErrorCode::InvalidArguments,
            Self::Io { .. } => ErrorCode::IoError,
        }
    }
}
impl Coded for super::help::HelpError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Fetch { .. } => ErrorCode::ServerUnreachable,
            Self::Cache { .. } => ErrorCode::IoError,
            Self::Index(_) => ErrorCode::InvalidData,
            Self::Invalid(_) => ErrorCode::InvalidArguments,
        }
    }
}
impl Coded for super::guide::GuideError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Invalid(_) => ErrorCode::InvalidArguments,
        }
    }
}
// A string newtype that mixes caller mistakes, server failures and local failures: no variant
// says which, so the honest category is `unclassified` until the type gains variants.
impl Coded for super::db::DbError {
    fn code(&self) -> ErrorCode {
        ErrorCode::Unclassified
    }
}
// Every failure of the type check is about running the compiler or writing its scratch files.
impl Coded for super::types::CheckError {
    fn code(&self) -> ErrorCode {
        ErrorCode::IoError
    }
}
impl Coded for super::deploy::DeployError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Baseline(error) => error.code(),
            Self::Working { .. }
            | Self::Server { .. }
            | Self::UnknownPlaceholder { .. }
            | Self::PlaceholderNotText { .. } => ErrorCode::InvalidData,
            Self::ParseUnavailable { source, .. } | Self::Import { source, .. } => source.code(),
            Self::ParseFailed(_) => ErrorCode::GateFailed,
            Self::Conflicts(_) => ErrorCode::ServerConflict,
            Self::Call { .. } => ErrorCode::ServerError,
            Self::NotKept(_) => ErrorCode::NotVerified,
            Self::AfterImport { source, .. } => source.code(),
            Self::Unrecorded { failure, .. } => failure.code(),
        }
    }
}
impl Coded for super::imports::ImportError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Remote(error) => error.code(),
            Self::Invalid(_) => ErrorCode::InvalidData,
        }
    }
}
impl Coded for super::search::SearchError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Invalid(_) => ErrorCode::InvalidArguments,
            Self::Remote(error) => error.code(),
            Self::Reply(_) => ErrorCode::InvalidData,
        }
    }
}

impl Coded for super::entity_get::GetError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Invalid(_) => ErrorCode::InvalidArguments,
            Self::Resolve(error) => error.code(),
            Self::NotOnServer(_) => ErrorCode::UnknownEntity,
            Self::Remote(error) => error.code(),
        }
    }
}

impl Coded for super::adopt::AdoptError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Export { .. }
            | Self::Repository { .. }
            | Self::Base { .. }
            | Self::AlreadyExists { .. } => ErrorCode::IoError,
            Self::Take(_) => ErrorCode::InvalidArguments,
            Self::Write(error) => error.code(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn strings_are_unique_and_serialize_as_declared() {
        let strings: BTreeSet<_> = ErrorCode::ALL.iter().map(|code| code.as_str()).collect();
        assert_eq!(strings.len(), ErrorCode::ALL.len());
        for code in ErrorCode::ALL {
            assert_eq!(
                serde_json::to_string(&code).unwrap(),
                format!("\"{}\"", code.as_str())
            );
        }
    }

    #[test]
    fn foundational_codes_match_their_variants() {
        assert_eq!(
            super::super::lock::LockError::Held {
                holder: String::new()
            }
            .code(),
            ErrorCode::WorkspaceLocked
        );
        assert_eq!(
            super::super::workspace::WorkspaceError::UnknownEntity {
                name: String::new()
            }
            .code(),
            ErrorCode::UnknownEntity
        );
        assert_eq!(
            super::super::entity_key::EntityKey::parse("bad")
                .unwrap_err()
                .code(),
            ErrorCode::InvalidArguments
        );
    }

    use std::fmt::Debug;
    use std::path::PathBuf;

    use crate::core::server::{Method, ServerError};

    /// A variant's mapping, with the variant named in the failure message.
    fn is<E: Coded + Debug>(error: E, expected: ErrorCode) {
        assert_eq!(error.code(), expected, "{error:?}");
    }

    fn text() -> String {
        String::new()
    }

    fn path() -> PathBuf {
        PathBuf::new()
    }

    fn transport() -> ServerError {
        ServerError::Transport {
            method: Method::Get,
            url: text(),
            why: text(),
        }
    }

    fn http() -> ServerError {
        ServerError::Http {
            method: Method::Get,
            status: 500,
            url: text(),
            body: text(),
        }
    }

    #[test]
    fn lock_error() {
        use crate::core::lock::LockError;
        is(
            LockError::Held { holder: text() },
            ErrorCode::WorkspaceLocked,
        );
        is(
            LockError::Io {
                path: path(),
                why: text(),
            },
            ErrorCode::IoError,
        );
        is(
            LockError::Recovery { message: text() },
            ErrorCode::RollbackFailed,
        );
    }

    #[test]
    fn transaction_error() {
        use crate::core::transaction::TransactionError;
        is(
            TransactionError::Invalid(text()),
            ErrorCode::InvalidArguments,
        );
        is(TransactionError::Stale(text()), ErrorCode::StalePlan);
        is(
            TransactionError::Io {
                path: path(),
                why: text(),
            },
            ErrorCode::IoError,
        );
        is(
            TransactionError::Failed {
                why: text(),
                rolled_back: true,
                journal: None,
                leftover: vec![],
            },
            ErrorCode::IoError,
        );
        is(
            TransactionError::Failed {
                why: text(),
                rolled_back: false,
                journal: Some(path()),
                leftover: vec![],
            },
            ErrorCode::RollbackFailed,
        );
    }

    #[test]
    fn server_error() {
        is(ServerError::InvalidUrl(text()), ErrorCode::InvalidArguments);
        is(transport(), ErrorCode::ServerUnreachable);
        is(http(), ErrorCode::ServerError);
        is(
            ServerError::Rejected {
                url: text(),
                body: text(),
            },
            ErrorCode::ServerError,
        );
        is(
            ServerError::UnsupportedCharset(text()),
            ErrorCode::ServerError,
        );
        is(ServerError::InvalidUtf8 { at: 0 }, ErrorCode::ServerError);
        is(
            ServerError::InvalidResponse {
                url: text(),
                why: text(),
            },
            ErrorCode::ServerError,
        );
    }

    #[test]
    fn workspace_error() {
        use crate::core::workspace::WorkspaceError;
        is(
            WorkspaceError::UnknownEntity { name: text() },
            ErrorCode::UnknownEntity,
        );
        is(
            WorkspaceError::Ambiguous {
                name: text(),
                found: vec![],
            },
            ErrorCode::Ambiguous,
        );
        is(
            WorkspaceError::InvalidCallTarget { name: text() },
            ErrorCode::InvalidArguments,
        );
        is(
            WorkspaceError::Io {
                path: path(),
                why: text(),
            },
            ErrorCode::IoError,
        );
    }

    #[test]
    fn config_error() {
        use crate::core::config::ConfigError;
        is(
            ConfigError::NotFound { from: path() },
            ErrorCode::InvalidData,
        );
        is(
            ConfigError::Unreadable {
                path: path(),
                why: text(),
            },
            ErrorCode::IoError,
        );
        is(
            ConfigError::Invalid {
                path: path(),
                why: text(),
            },
            ErrorCode::InvalidData,
        );
        is(
            ConfigError::NoProjects { path: path() },
            ErrorCode::InvalidData,
        );
        is(
            ConfigError::DuplicateProject { name: text() },
            ErrorCode::InvalidData,
        );
        is(ConfigError::BlankProjectName, ErrorCode::InvalidData);
        is(ConfigError::BlankCheckName, ErrorCode::InvalidData);
        is(
            ConfigError::EmptyCheckCommand { name: text() },
            ErrorCode::InvalidData,
        );
        is(ConfigError::EmptyTypesCompiler, ErrorCode::InvalidData);
        is(
            ConfigError::EscapingPath {
                what: "path",
                value: text(),
            },
            ErrorCode::InvalidData,
        );
        is(
            ConfigError::UnknownDependency {
                project: text(),
                missing: text(),
            },
            ErrorCode::InvalidData,
        );
        is(
            ConfigError::DependencyCycle { names: vec![] },
            ErrorCode::InvalidData,
        );
    }

    #[test]
    fn profile_error() {
        use crate::core::profile::ProfileError;
        is(
            ProfileError::InvalidName(text()),
            ErrorCode::InvalidArguments,
        );
        is(
            ProfileError::Missing {
                name: text(),
                searched: vec![],
            },
            ErrorCode::InvalidData,
        );
        is(
            ProfileError::Unreadable {
                path: path(),
                why: text(),
            },
            ErrorCode::InvalidData,
        );
        is(
            ProfileError::Invalid {
                path: path(),
                why: text(),
            },
            ErrorCode::InvalidData,
        );
        is(
            ProfileError::Incomplete {
                source: text(),
                missing: vec![],
            },
            ErrorCode::InvalidData,
        );
    }

    #[test]
    fn rename_error() {
        use crate::core::rename::RenameError;
        is(
            RenameError::InvalidNew { message: text() },
            ErrorCode::InvalidArguments,
        );
        is(
            RenameError::InvalidOld { message: text() },
            ErrorCode::InvalidArguments,
        );
        is(
            RenameError::Same { name: text() },
            ErrorCode::InvalidArguments,
        );
        is(
            RenameError::Nested {
                old: text(),
                new: text(),
            },
            ErrorCode::InvalidArguments,
        );
        is(
            RenameError::Unknown { old: text() },
            ErrorCode::UnknownEntity,
        );
        is(
            RenameError::Ambiguous {
                old: text(),
                files: vec![],
            },
            ErrorCode::Ambiguous,
        );
        is(
            RenameError::Exists { conflicts: vec![] },
            ErrorCode::AlreadyExists,
        );
        is(
            RenameError::DatabaseHalf {
                shapes: vec![],
                unsure: vec![],
            },
            ErrorCode::GuardRefused,
        );
        is(
            RenameError::ServiceScope { message: text() },
            ErrorCode::GuardRefused,
        );
        is(
            RenameError::TableScope { message: text() },
            ErrorCode::GuardRefused,
        );
        is(
            RenameError::WouldDesync { entities: vec![] },
            ErrorCode::GuardRefused,
        );
        is(
            RenameError::GatesFail { gates: vec![] },
            ErrorCode::GateFailed,
        );
        is(RenameError::Stale { path: path() }, ErrorCode::StalePlan);
        is(
            RenameError::PlanChanged {
                expected: text(),
                actual: text(),
            },
            ErrorCode::StalePlan,
        );
        is(
            RenameError::RollbackFailed {
                original: Box::new(RenameError::Stale { path: path() }),
                leftover: vec![],
            },
            ErrorCode::RollbackFailed,
        );
        is(
            RenameError::Io {
                path: path(),
                why: text(),
            },
            ErrorCode::IoError,
        );
        is(
            RenameError::Apply {
                path: path(),
                why: text(),
            },
            ErrorCode::IoError,
        );
        is(
            RenameError::Xml {
                path: path(),
                why: text(),
            },
            ErrorCode::InvalidData,
        );
        is(
            RenameError::Splice {
                path: path(),
                why: text(),
            },
            ErrorCode::InvalidData,
        );
        is(
            RenameError::InvalidLedger {
                path: path(),
                why: text(),
            },
            ErrorCode::InvalidData,
        );
        is(
            RenameError::Unreadable { files: vec![] },
            ErrorCode::InvalidData,
        );
    }

    #[test]
    fn refusal() {
        use crate::core::push::Refusal;
        is(Refusal::DeletedOnServer, ErrorCode::ServerConflict);
        is(
            Refusal::UnknownAncestor { server: text() },
            ErrorCode::ServerConflict,
        );
        is(
            Refusal::Conflict {
                server: text(),
                baseline: text(),
            },
            ErrorCode::ServerConflict,
        );
    }

    #[test]
    fn push_error() {
        use crate::core::baseline::BaselineError;
        use crate::core::normalise::NormaliseError;
        use crate::core::push::PushError;
        is(
            PushError::Working(NormaliseError::Malformed(text())),
            ErrorCode::InvalidData,
        );
        is(
            PushError::Server(NormaliseError::Malformed(text())),
            ErrorCode::InvalidData,
        );
        // Delegation: the wrapped error decides.
        is(PushError::Remote(transport()), ErrorCode::ServerUnreachable);
        is(PushError::Remote(http()), ErrorCode::ServerError);
        is(PushError::Unverified(transport()), ErrorCode::NotVerified);
        is(
            PushError::NotKept {
                sent: text(),
                read_back: None,
            },
            ErrorCode::NotVerified,
        );
        is(
            PushError::Baseline(BaselineError::Io {
                path: path(),
                why: text(),
            }),
            ErrorCode::IoError,
        );
        is(
            PushError::Baseline(BaselineError::Invalid {
                path: path(),
                why: text(),
            }),
            ErrorCode::InvalidData,
        );
    }

    #[test]
    fn baseline_error() {
        use crate::core::baseline::BaselineError;
        is(
            BaselineError::Io {
                path: path(),
                why: text(),
            },
            ErrorCode::IoError,
        );
        is(
            BaselineError::Invalid {
                path: path(),
                why: text(),
            },
            ErrorCode::InvalidData,
        );
        is(
            BaselineError::Missing {
                collection: text(),
                name: text(),
            },
            ErrorCode::InvalidData,
        );
    }

    #[test]
    fn delete_error() {
        use crate::core::entity_delete::DeleteError;
        is(
            DeleteError::Remote {
                entity: text(),
                why: transport(),
            },
            ErrorCode::ServerUnreachable,
        );
        is(
            DeleteError::Remote {
                entity: text(),
                why: http(),
            },
            ErrorCode::ServerError,
        );
        is(DeleteError::Target(text()), ErrorCode::InvalidArguments);
        is(
            DeleteError::Ledger {
                path: path(),
                why: text(),
            },
            ErrorCode::InvalidData,
        );
        is(
            DeleteError::Write {
                path: path(),
                why: text(),
            },
            ErrorCode::IoError,
        );
        is(DeleteError::Backup(text()), ErrorCode::IoError);
    }

    #[test]
    fn carry_error() {
        use crate::core::entity_carry::CarryError;
        is(CarryError::Ledger { why: text() }, ErrorCode::InvalidData);
        is(
            CarryError::Arguments { why: text() },
            ErrorCode::InvalidArguments,
        );
    }

    #[test]
    fn export_error() {
        use crate::core::export::ExportError;
        is(
            ExportError::Remote(transport()),
            ErrorCode::ServerUnreachable,
        );
        is(ExportError::Remote(http()), ErrorCode::ServerError);
        is(ExportError::Invalid(text()), ErrorCode::InvalidArguments);
    }

    #[test]
    fn backup_error() {
        use crate::core::backup::BackupError;
        is(
            BackupError::Remote {
                entity: text(),
                why: transport(),
            },
            ErrorCode::ServerUnreachable,
        );
        is(
            BackupError::Remote {
                entity: text(),
                why: http(),
            },
            ErrorCode::ServerError,
        );
        is(
            BackupError::Io {
                path: path(),
                why: text(),
            },
            ErrorCode::IoError,
        );
        is(
            BackupError::Unreadable { entity: text() },
            ErrorCode::InvalidData,
        );
        is(
            BackupError::Invalid {
                path: path(),
                why: text(),
            },
            ErrorCode::InvalidData,
        );
        is(
            BackupError::NoSuchSet { id: text() },
            ErrorCode::UnknownEntity,
        );
        is(BackupError::Plan(text()), ErrorCode::GuardRefused);
    }

    #[test]
    fn relocate_error() {
        use crate::core::relocate::{Member, RelocateError};
        is(RelocateError::Invalid(text()), ErrorCode::InvalidArguments);
        is(
            RelocateError::Unknown { name: text() },
            ErrorCode::UnknownEntity,
        );
        is(
            RelocateError::NotDeclared {
                entity: text(),
                member: Member::Service,
                name: text(),
            },
            ErrorCode::UnknownEntity,
        );
        is(
            RelocateError::Unreadable { files: vec![] },
            ErrorCode::InvalidData,
        );
        is(
            RelocateError::Xml {
                path: path(),
                why: text(),
            },
            ErrorCode::InvalidData,
        );
        is(
            RelocateError::Inherited {
                entity: text(),
                name: text(),
                declared_on: text(),
            },
            ErrorCode::GuardRefused,
        );
        is(RelocateError::Refused(text()), ErrorCode::GuardRefused);
        is(
            RelocateError::Exists { conflicts: vec![] },
            ErrorCode::AlreadyExists,
        );
        is(
            RelocateError::Apply {
                path: path(),
                why: text(),
            },
            ErrorCode::IoError,
        );
        is(RelocateError::Verification(text()), ErrorCode::StalePlan);
        is(
            RelocateError::Write(crate::core::transaction::TransactionError::Failed {
                why: text(),
                rolled_back: false,
                journal: None,
                leftover: vec![],
            }),
            ErrorCode::RollbackFailed,
        );
    }

    #[test]
    fn new_block_error() {
        use crate::core::newblock::NewBlockError;
        is(NewBlockError::Invalid(text()), ErrorCode::InvalidArguments);
        is(NewBlockError::Exists(vec![]), ErrorCode::AlreadyExists);
        is(
            NewBlockError::Io {
                path: path(),
                why: text(),
            },
            ErrorCode::IoError,
        );
        is(
            NewBlockError::Write(crate::core::transaction::TransactionError::Failed {
                why: text(),
                rolled_back: false,
                journal: None,
                leftover: vec![],
            }),
            ErrorCode::RollbackFailed,
        );
    }

    #[test]
    fn key_error() {
        use crate::core::entity_key::EntityKey;
        is(
            EntityKey::parse("bad").unwrap_err(),
            ErrorCode::InvalidArguments,
        );
        is(
            EntityKey::new("Things", "").unwrap_err(),
            ErrorCode::InvalidArguments,
        );
    }

    #[test]
    fn types_error() {
        use crate::core::types::TypesError;
        use crate::core::workspace::WorkspaceError;
        is(
            TypesError::Workspace(WorkspaceError::UnknownEntity { name: text() }),
            ErrorCode::UnknownEntity,
        );
        is(
            TypesError::Remote(transport()),
            ErrorCode::ServerUnreachable,
        );
        is(TypesError::Platform(text()), ErrorCode::InvalidData);
    }

    #[test]
    fn check_error() {
        // A string newtype: every failure is about running the compiler or its scratch files.
        use crate::core::types::CheckError;
        is(CheckError(text()), ErrorCode::IoError);
    }

    #[test]
    fn retemplate_error() {
        use crate::core::retemplate::RetemplateError;
        is(
            RetemplateError::Invalid(text()),
            ErrorCode::InvalidArguments,
        );
        is(
            RetemplateError::Unknown { name: text() },
            ErrorCode::UnknownEntity,
        );
        is(
            RetemplateError::Unreadable { files: vec![] },
            ErrorCode::InvalidData,
        );
        is(
            RetemplateError::Xml {
                path: path(),
                why: text(),
            },
            ErrorCode::InvalidData,
        );
        is(
            RetemplateError::Loss { reasons: vec![] },
            ErrorCode::GuardRefused,
        );
        is(
            RetemplateError::Apply {
                path: path(),
                why: text(),
            },
            ErrorCode::IoError,
        );
    }

    #[test]
    fn copy_error() {
        use crate::core::datatable_copy::CopyError;
        is(CopyError::Arguments(text()), ErrorCode::InvalidArguments);
        is(CopyError::Ledger(text()), ErrorCode::InvalidData);
        is(CopyError::Refused(text()), ErrorCode::GuardRefused);
        is(CopyError::Server(transport()), ErrorCode::ServerUnreachable);
        is(CopyError::Server(http()), ErrorCode::ServerError);
    }

    #[test]
    fn catalog_error() {
        use crate::core::catalog::CatalogError;
        is(CatalogError::Invalid(text()), ErrorCode::InvalidArguments);
        is(
            CatalogError::UnknownEntity(text()),
            ErrorCode::UnknownEntity,
        );
        is(CatalogError::Ambiguous(text()), ErrorCode::Ambiguous);
    }

    #[test]
    fn table_error() {
        use crate::core::config_table::TableError;
        is(
            TableError::Remote(transport()),
            ErrorCode::ServerUnreachable,
        );
        is(
            TableError::PartlyRestored {
                why: http(),
                left: vec![],
            },
            ErrorCode::ServerError,
        );
        is(TableError::Shape(text()), ErrorCode::InvalidData);
        is(
            TableError::Backup {
                path: text(),
                why: text(),
            },
            ErrorCode::InvalidData,
        );
        is(TableError::Repository(text()), ErrorCode::InvalidData);
        is(
            TableError::WrongBackup {
                expected: text(),
                found: text(),
            },
            ErrorCode::GuardRefused,
        );
        is(TableError::NoPrimaryKey, ErrorCode::GuardRefused);
        is(
            TableError::KeyMismatch {
                backup: vec![],
                server: vec![],
            },
            ErrorCode::GuardRefused,
        );
        is(TableError::BadKey(text()), ErrorCode::GuardRefused);
        is(TableError::NotRestored(vec![]), ErrorCode::NotVerified);
    }

    #[test]
    fn repo_error() {
        use crate::core::repo::RepoError;
        is(RepoError::Remote(transport()), ErrorCode::ServerUnreachable);
        is(RepoError::Shape(text()), ErrorCode::InvalidData);
        is(RepoError::Invalid(text()), ErrorCode::InvalidArguments);
        is(
            RepoError::Local {
                path: path(),
                why: text(),
            },
            ErrorCode::IoError,
        );
    }

    #[test]
    fn settings_error() {
        use crate::core::settings::SettingsError;
        is(
            SettingsError::Remote(transport()),
            ErrorCode::ServerUnreachable,
        );
        is(SettingsError::Remote(http()), ErrorCode::ServerError);
        is(SettingsError::Shape(text()), ErrorCode::InvalidData);
        is(SettingsError::Invalid(text()), ErrorCode::InvalidArguments);
    }

    #[test]
    fn extension_error() {
        use crate::core::extensions::ExtensionError;
        is(
            ExtensionError::Remote(transport()),
            ErrorCode::ServerUnreachable,
        );
        is(ExtensionError::Shape(text()), ErrorCode::InvalidData);
        is(ExtensionError::Invalid(text()), ErrorCode::InvalidArguments);
    }

    #[test]
    fn logs_error() {
        use crate::core::logs::LogsError;
        is(LogsError::Remote(transport()), ErrorCode::ServerUnreachable);
        is(LogsError::Shape(text()), ErrorCode::InvalidData);
        is(LogsError::Invalid(text()), ErrorCode::InvalidArguments);
    }

    #[test]
    fn impact_error() {
        use crate::core::impact::ImpactError;
        use crate::core::workspace::WorkspaceError;
        is(
            ImpactError::Entity(WorkspaceError::UnknownEntity { name: text() }),
            ErrorCode::UnknownEntity,
        );
        is(
            ImpactError::Entity(WorkspaceError::Ambiguous {
                name: text(),
                found: vec![text()],
            }),
            ErrorCode::Ambiguous,
        );
    }

    #[test]
    fn package_error() {
        use crate::core::package::PackageError;
        is(PackageError::Invalid(text()), ErrorCode::InvalidArguments);
        is(
            PackageError::Io {
                path: path(),
                why: text(),
            },
            ErrorCode::IoError,
        );
    }

    #[test]
    fn help_error() {
        use crate::core::help::HelpError;
        is(
            HelpError::Fetch {
                url: text(),
                why: text(),
            },
            ErrorCode::ServerUnreachable,
        );
        is(
            HelpError::Cache {
                path: path(),
                why: text(),
            },
            ErrorCode::IoError,
        );
        is(HelpError::Index(text()), ErrorCode::InvalidData);
        is(HelpError::Invalid(text()), ErrorCode::InvalidArguments);
    }

    #[test]
    fn guide_error() {
        use crate::core::guide::GuideError;
        is(GuideError::Invalid(text()), ErrorCode::InvalidArguments);
    }

    #[test]
    fn search_and_entity_get_errors() {
        use crate::core::entity_get::GetError;
        use crate::core::search::SearchError;
        is(SearchError::Invalid(text()), ErrorCode::InvalidArguments);
        is(SearchError::Reply(text()), ErrorCode::InvalidData);
        is(GetError::Invalid(text()), ErrorCode::InvalidArguments);
        is(
            GetError::NotOnServer(crate::core::entity_key::EntityKey::new("Things", "T").unwrap()),
            ErrorCode::UnknownEntity,
        );
    }

    #[test]
    fn adopt_error() {
        use crate::core::adopt::AdoptError;
        is(
            AdoptError::Export {
                path: path(),
                why: text(),
            },
            ErrorCode::IoError,
        );
        is(
            AdoptError::Repository {
                path: path(),
                why: text(),
            },
            ErrorCode::IoError,
        );
        is(
            AdoptError::Write(crate::core::transaction::TransactionError::Stale(text())),
            ErrorCode::StalePlan,
        );
    }

    #[test]
    fn db_error() {
        // A string newtype that mixes caller, server and local failures: unclassified until it
        // gains variants.
        use crate::core::db::DbError;
        is(DbError(text()), ErrorCode::Unclassified);
    }

    #[test]
    fn deploy_error() {
        use crate::core::baseline::BaselineError;
        use crate::core::deploy::DeployError;
        let io = || BaselineError::Io {
            path: path(),
            why: text(),
        };
        is(DeployError::Baseline(io()), ErrorCode::IoError);
        is(
            DeployError::Working {
                collection: text(),
                name: text(),
                why: text(),
            },
            ErrorCode::InvalidData,
        );
        is(
            DeployError::Server {
                collection: text(),
                name: text(),
                why: text(),
            },
            ErrorCode::InvalidData,
        );
        is(
            DeployError::UnknownPlaceholder {
                project: text(),
                key: text(),
            },
            ErrorCode::InvalidData,
        );
        is(
            DeployError::PlaceholderNotText {
                project: text(),
                key: text(),
            },
            ErrorCode::InvalidData,
        );
        is(
            DeployError::ParseUnavailable {
                entity: text(),
                service: text(),
                source: transport(),
            },
            ErrorCode::ServerUnreachable,
        );
        is(DeployError::ParseFailed(vec![]), ErrorCode::GateFailed);
        is(DeployError::Conflicts(vec![]), ErrorCode::ServerConflict);
        is(
            DeployError::Import {
                project: text(),
                source: http(),
                imported: vec![],
            },
            ErrorCode::ServerError,
        );
        is(
            DeployError::Call {
                project: text(),
                target: text(),
                service: text(),
                why: text(),
                imported: vec![],
            },
            ErrorCode::ServerError,
        );
        is(DeployError::NotKept(Box::default()), ErrorCode::NotVerified);
        // Delegation through the nested errors.
        is(
            DeployError::AfterImport {
                imported: vec![],
                source: Box::new(DeployError::Conflicts(vec![])),
            },
            ErrorCode::ServerConflict,
        );
        is(
            DeployError::Unrecorded {
                failure: Box::new(DeployError::ParseFailed(vec![])),
                why: io(),
                imported: vec![],
            },
            ErrorCode::GateFailed,
        );
    }

    #[test]
    fn import_error() {
        use crate::core::imports::ImportError;
        is(
            ImportError::Remote(transport()),
            ErrorCode::ServerUnreachable,
        );
        is(ImportError::Remote(http()), ErrorCode::ServerError);
        is(ImportError::Invalid(text()), ErrorCode::InvalidData);
    }
}
