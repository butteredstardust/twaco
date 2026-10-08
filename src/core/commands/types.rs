//! The command policy around generated TypeScript declarations.

use super::{lock_workspace, Access, Effects, Notices};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::{lock, profile, types};

/// The operation a types request performs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypesAction {
    Generate,
    Check,
    Platform,
    Invalid(String),
}

/// The arguments that affect generated declarations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypesRequest {
    pub action: TypesAction,
    pub profile: String,
    /// The lock-holder label the calling surface has historically shown.
    pub lock_label: &'static str,
}

/// One completed types operation, before either adapter projects it to its wire format.
#[derive(Debug)]
pub enum TypesOutcome {
    Generated(types::Outcome),
    Checked(types::CheckOutcome),
    Platform(types::PlatformOutcome),
}

impl TypesOutcome {
    /// Each current types operation writes generated declarations or the platform cache.
    pub const fn effects(&self) -> Effects {
        match self {
            Self::Generated(_) | Self::Checked(_) => Effects::new(Access::Write, Access::None),
            Self::Platform(_) => Effects::new(Access::Write, Access::Read),
        }
    }
}

/// A failure before a types outcome could be produced.
#[derive(Debug, thiserror::Error)]
pub enum TypesCommandError {
    #[error("{0}")]
    Lock(lock::LockError),
    #[error("{0}")]
    Arguments(String),
    #[error("{0}")]
    Profile(profile::ProfileError),
    #[error("{0}")]
    Types(types::TypesError),
    #[error("{0}")]
    Check(types::CheckError),
}

impl Coded for TypesCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Lock(why) => why.code(),
            Self::Arguments(_) => ErrorCode::InvalidArguments,
            Self::Profile(why) => why.code(),
            Self::Types(why) => why.code(),
            Self::Check(why) => why.code(),
        }
    }
}

/// Run one types operation. All current operations write generated files, so the lock is taken
/// before loading a profile, discovering entities or invoking the compiler.
pub fn execute<R, F>(
    solution: &Solution,
    request: &TypesRequest,
    open: F,
    compiler: Option<&dyn types::CompilerRunner>,
    notices: &mut Notices,
) -> Result<TypesOutcome, TypesCommandError>
where
    R: types::Remote,
    F: FnOnce(profile::Profile) -> R,
{
    let _lock =
        lock_workspace(solution, request.lock_label, notices).map_err(TypesCommandError::Lock)?;
    match &request.action {
        TypesAction::Generate => types::write(solution)
            .map(TypesOutcome::Generated)
            .map_err(TypesCommandError::Types),
        TypesAction::Check => match compiler {
            Some(compiler) => types::check_with(solution, compiler),
            None => types::check(solution),
        }
        .map(TypesOutcome::Checked)
        .map_err(TypesCommandError::Check),
        TypesAction::Platform => {
            let profile = profile::load(&solution.root, &request.profile)
                .map_err(TypesCommandError::Profile)?;
            types::fetch_platform(&open(profile), solution)
                .map(TypesOutcome::Platform)
                .map_err(TypesCommandError::Types)
        }
        TypesAction::Invalid(why) => Err(TypesCommandError::Arguments(why.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::lock;
    use crate::core::server::Client;

    #[test]
    fn generation_locks_before_it_can_read_the_workspace() {
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-command-types-")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let held = lock::acquire(&root, "holder", &[]).unwrap();
        let request = TypesRequest {
            action: TypesAction::Generate,
            profile: "default".to_string(),
            lock_label: "types",
        };
        let error = execute(
            &solution,
            &request,
            Client::new,
            None,
            &mut Notices::default(),
        )
        .unwrap_err();
        assert!(matches!(error, TypesCommandError::Lock(_)));
        drop(held);
    }

    #[test]
    fn wrapped_errors_keep_their_codes() {
        let error = TypesCommandError::Lock(lock::LockError::Held {
            holder: "other command".to_string(),
        });
        assert_eq!(error.code(), ErrorCode::WorkspaceLocked);
        let error =
            TypesCommandError::Profile(profile::ProfileError::InvalidName("bad/name".to_string()));
        assert_eq!(error.code(), ErrorCode::InvalidArguments);
        let error = TypesCommandError::Arguments("bad action".to_string());
        assert_eq!(error.code(), ErrorCode::InvalidArguments);
    }
}
