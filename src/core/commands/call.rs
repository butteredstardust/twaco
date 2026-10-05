//! The command policy around one opaque service call.

use super::{Access, Effects, Mode, Notices};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::entity_key::ServiceTarget;
use crate::core::{logs, profile, server, workspace};
use serde_json::Value;
use std::fmt;
use std::time::Duration;

type Logged = Option<Result<Vec<(String, logs::Entry)>, logs::LogsError>>;

/// The arguments that affect an opaque service call.
#[derive(Clone, Debug)]
pub struct CallRequest {
    pub target: String,
    pub service: String,
    pub parameters: Value,
    pub timeout: Duration,
    pub mode: Mode,
    pub profile: String,
    pub with_logs: bool,
    /// The command line historically reports a missing profile before resolving its target.
    pub profile_before_target: bool,
}

/// The completed service call or its dry-run description.
#[derive(Debug)]
pub enum CallOutcome {
    Plan {
        target: ServiceTarget,
        effects: Effects,
    },
    Applied {
        target: ServiceTarget,
        reply: Option<Value>,
        logs: Logged,
        effects: Effects,
    },
}

impl CallOutcome {
    /// The access this operation used or may have used.
    pub const fn effects(&self) -> Effects {
        match self {
            Self::Plan { effects, .. } | Self::Applied { effects, .. } => *effects,
        }
    }

    /// The service target after repository shorthand was resolved.
    pub fn target(&self) -> &ServiceTarget {
        match self {
            Self::Plan { target, .. } | Self::Applied { target, .. } => target,
        }
    }
}

/// What an opaque service call needs from a server.
pub trait Remote: logs::Remote {
    fn call_service(
        &self,
        target: &ServiceTarget,
        service: &str,
        parameters: &Value,
        timeout: Duration,
    ) -> Result<Option<Value>, server::ServerError>;
}

impl Remote for server::Client {
    fn call_service(
        &self,
        target: &ServiceTarget,
        service: &str,
        parameters: &Value,
        timeout: Duration,
    ) -> Result<Option<Value>, server::ServerError> {
        self.call_service(target, service, parameters, timeout)
    }
}

/// A failure before a typed service-call outcome could be produced.
#[derive(Debug)]
pub enum CallCommandError {
    Profile(profile::ProfileError),
    Target(workspace::WorkspaceError),
    Call {
        target: ServiceTarget,
        error: server::ServerError,
        logs: Box<Logged>,
    },
}

impl fmt::Display for CallCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Profile(error) => error.fmt(f),
            Self::Target(error) => error.fmt(f),
            Self::Call { error, logs, .. } => {
                error.fmt(f)?;
                match &**logs {
                    Some(Ok(entries)) if !entries.is_empty() => {
                        let lines = entries
                            .iter()
                            .map(|(log, entry)| format!("{log}: {}", logs::line(entry)))
                            .collect::<Vec<_>>();
                        write!(f, "\nlogged during the call:\n{}", lines.join("\n"))
                    }
                    Some(Err(why)) => write!(f, "\n(the call's logs could not be read: {why})"),
                    _ => Ok(()),
                }
            }
        }
    }
}

impl std::error::Error for CallCommandError {}

impl Coded for CallCommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Profile(error) => error.code(),
            Self::Target(error) => error.code(),
            Self::Call { error, .. } => error.code(),
        }
    }
}

/// Describe or issue one service call. An opaque call changes only the server, so it never takes
/// the workspace lock.
pub fn execute<R, F>(
    solution: &Solution,
    request: &CallRequest,
    open: F,
    _: &mut Notices,
) -> Result<CallOutcome, CallCommandError>
where
    R: Remote,
    F: FnOnce(profile::Profile) -> R,
{
    let selected = if request.profile_before_target {
        Some(profile::load(&solution.root, &request.profile).map_err(CallCommandError::Profile)?)
    } else {
        None
    };
    let target = workspace::call_target(&workspace::discover(solution).entities, &request.target)
        .map_err(CallCommandError::Target)?;
    if matches!(request.mode, Mode::Plan) {
        return Ok(CallOutcome::Plan {
            target,
            effects: Effects::new(Access::Read, Access::None),
        });
    }
    let profile = match selected {
        Some(profile) => profile,
        None => {
            profile::load(&solution.root, &request.profile).map_err(CallCommandError::Profile)?
        }
    };
    let remote = open(profile);
    let started = logs::now_ms();
    let called = remote.call_service(
        &target,
        &request.service,
        &request.parameters,
        request.timeout,
    );
    let logged = request.with_logs.then(|| {
        logs::during_call(
            &remote,
            started,
            logs::now_ms(),
            logs::Wait::default(),
            &logs::now_ms,
            &std::thread::sleep,
        )
    });
    let reply = match called {
        Ok(reply) => reply,
        Err(error) => {
            return Err(CallCommandError::Call {
                target,
                error,
                logs: Box::new(logged),
            })
        }
    };
    Ok(CallOutcome::Applied {
        target,
        reply,
        logs: logged,
        effects: Effects::new(Access::Read, Access::Write),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn solution() -> (PathBuf, Solution) {
        let root = std::env::temp_dir().join(format!(
            "twaco-command-call-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (root, solution)
    }

    #[test]
    fn a_plan_resolves_the_target_without_loading_a_profile_or_calling() {
        let (root, solution) = solution();
        let request = CallRequest {
            target: "Things/Outside".to_string(),
            service: "Read".to_string(),
            parameters: serde_json::json!({}),
            timeout: Duration::from_secs(1),
            mode: Mode::Plan,
            profile: "missing".to_string(),
            with_logs: false,
            profile_before_target: false,
        };
        let outcome = execute::<server::Client, _>(
            &solution,
            &request,
            |_| panic!("a plan must not open a server"),
            &mut Notices::default(),
        )
        .unwrap();
        assert_eq!(outcome.effects(), Effects::new(Access::Read, Access::None));
        assert_eq!(outcome.target().to_string(), "Things/Outside");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_command_line_call_still_loads_its_profile_before_target_resolution() {
        let (root, solution) = solution();
        let request = CallRequest {
            target: "not a target".to_string(),
            service: "Read".to_string(),
            parameters: serde_json::json!({}),
            timeout: Duration::from_secs(1),
            mode: Mode::Apply,
            profile: "missing".to_string(),
            with_logs: false,
            profile_before_target: true,
        };
        let error = execute::<server::Client, _>(
            &solution,
            &request,
            |_| panic!("a missing profile must stop before opening a server"),
            &mut Notices::default(),
        )
        .unwrap_err();
        assert!(matches!(error, CallCommandError::Profile(_)));
        assert_eq!(error.code(), ErrorCode::InvalidData);
        std::fs::remove_dir_all(root).unwrap();
    }
}
