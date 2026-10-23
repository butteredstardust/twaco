//! The command policy around one opaque service call.

use super::{Access, Effects, Mode, Notices};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::entity::key::ServiceTarget;
use crate::core::{logs, profile, read_only, server, workspace};
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
    /// Call as this `[[test.user]]`, with the profile's `test_password`.
    pub as_user: Option<String>,
    /// Make the call in this language (`de`, or a preference list `fr,de`). Only with `as_user`:
    /// ThingWorx takes a call's language from the user's stored preference, so the test user's
    /// preference is set for the call and put back after it.
    pub lang: Option<String>,
    /// The caller left the choice to twaco: a plan becomes a call when the service only reads
    /// (and no `lang` makes the call write the user's preference).
    pub run_if_read_only: bool,
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
        /// Set when a plan ran because the service only reads.
        read_only: Option<read_only::Reason>,
    },
}

impl CallOutcome {
    /// The access this operation used or may have used.
    #[cfg(test)]
    pub(crate) const fn effects(&self) -> Effects {
        match self {
            Self::Plan { effects, .. } | Self::Applied { effects, .. } => *effects,
        }
    }

    /// The service target after repository shorthand was resolved.
    pub(crate) fn target(&self) -> &ServiceTarget {
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
    TestUser(crate::core::testing::TestError),
    /// `--lang` without `--as`, or a language that is not a language tag list.
    Language(String),
    /// The language could not be read or set, so nothing was called.
    SetLanguage {
        user: String,
        error: server::ServerError,
    },
    /// The call ran (its outcome is in `call`) but the user's language could not be put back.
    RestoreLanguage {
        user: String,
        previous: String,
        error: server::ServerError,
        call: Box<Result<CallOutcome, CallCommandError>>,
    },
    Target(workspace::WorkspaceError),
    /// An argument the repository's signature of the service says cannot be right.
    Arguments(String),
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
            Self::TestUser(error) => error.fmt(f),
            Self::Language(why) => f.write_str(why),
            Self::SetLanguage { user, error } => {
                write!(
                    f,
                    "could not set the language of {user}, so nothing was called: {error}"
                )
            }
            Self::RestoreLanguage {
                user,
                previous,
                error,
                call,
            } => write!(
                f,
                "the call ran ({}), but the language of {user} could not be put back to \
                 {previous:?}: {error}; put it back with `twaco call Users/{user} \
                 SetLanguagePreferences '{{\"preferences\":{previous:?}}}'`",
                if call.is_ok() {
                    "it succeeded"
                } else {
                    "it failed"
                }
            ),
            Self::Target(error) => error.fmt(f),
            Self::Arguments(why) => f.write_str(why),
            Self::Call { error, logs, .. } => {
                error.fmt(f)?;
                if matches!(error, server::ServerError::Http { status: 406, .. }) {
                    f.write_str("
(the server could not read the arguments: check them against the service's parameters, `twaco catalog <Entity>.<Service>`)")?;
                }
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
            Self::TestUser(error) => error.code(),
            Self::Language(_) => ErrorCode::InvalidArguments,
            Self::SetLanguage { error, .. } | Self::RestoreLanguage { error, .. } => error.code(),
            Self::Target(error) => error.code(),
            Self::Arguments(_) => ErrorCode::InvalidArguments,
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
    F: Fn(profile::Profile) -> R,
{
    if request.lang.is_some() && request.as_user.is_none() {
        return Err(CallCommandError::Language(
            "--lang needs --as <test user>: ThingWorx takes a call's language from the calling \
             user's stored preference, which twaco changes only for a declared test user"
                .to_string(),
        ));
    }
    if let Some(lang) = &request.lang {
        valid_language(lang)?;
    }
    let selected = if request.profile_before_target {
        Some(profile::load(&solution.root, &request.profile).map_err(CallCommandError::Profile)?)
    } else {
        None
    };
    let target = workspace::call_target(&workspace::discover(solution).entities, &request.target)
        .map_err(CallCommandError::Target)?;
    check_arguments(solution, &target, &request.service, &request.parameters)?;
    let read_only =
        (matches!(request.mode, Mode::Plan) && request.run_if_read_only && request.lang.is_none())
            .then(|| read_only::reason(solution, &target, &request.service))
            .flatten();
    if matches!(request.mode, Mode::Plan) && read_only.is_none() {
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
    let signed_in = match &request.as_user {
        None => profile.clone(),
        Some(user) => as_test_user(solution, &profile, user)?,
    };
    let (Some(lang), Some(user)) = (&request.lang, &request.as_user) else {
        return call(request, target, &open(signed_in)).map(|outcome| match outcome {
            CallOutcome::Applied {
                target,
                reply,
                logs,
                ..
            } if read_only.is_some() => CallOutcome::Applied {
                target,
                reply,
                logs,
                effects: Effects::new(Access::Read, Access::Read),
                read_only,
            },
            outcome => outcome,
        });
    };
    // The profile's own account changes the test user's preference; the call runs as the test
    // user; the old preference goes back whatever the call did.
    let admin = open(profile);
    let unset = |error| CallCommandError::SetLanguage {
        user: user.clone(),
        error,
    };
    let previous = language_of(&admin, user, request.timeout).map_err(unset)?;
    set_language(&admin, user, lang, request.timeout).map_err(unset)?;
    let called = call(request, target, &open(signed_in));
    match set_language(&admin, user, &previous, request.timeout) {
        Ok(()) => called,
        Err(error) => Err(CallCommandError::RestoreLanguage {
            user: user.clone(),
            previous,
            error,
            call: Box::new(called),
        }),
    }
}

/// Refuse arguments the repository's own signature of the service rules out: a name it does not
/// declare, or a JSON value that cannot be its base type. A service the repository cannot resolve
/// is the server's to judge.
fn check_arguments(
    solution: &Solution,
    target: &ServiceTarget,
    service: &str,
    parameters: &Value,
) -> Result<(), CallCommandError> {
    let empty = serde_json::Map::new();
    let given = match parameters {
        Value::Null => &empty,
        Value::Object(given) => given,
        // Not a parameter map: the server's to refuse.
        _ => return Ok(()),
    };
    let (model, _) = crate::core::types::load_model(solution);
    let Some(entity) = model.entities.iter().find(|e| match target {
        ServiceTarget::Thing(name) => e.collection == "Things" && &e.name == name,
        ServiceTarget::Entity(key) => e.collection == key.collection() && e.name == key.name(),
    }) else {
        return Ok(());
    };
    let platform = crate::core::types::platform::cached(solution);
    let found =
        crate::core::catalog::lookup_service(entity, &model.entities, platform.as_ref(), service);
    let Some(signature) = found.service else {
        return Ok(());
    };
    if let Some(missing) = signature
        .parameters
        .iter()
        .find(|p| p.required && p.default.is_none() && !given.contains_key(&p.name))
    {
        return Err(CallCommandError::Arguments(format!(
            "{service} requires {} ({})",
            missing.name, missing.value.base_type
        )));
    }
    for (name, value) in given {
        let Some(parameter) = signature.parameters.iter().find(|p| &p.name == name) else {
            let declared: Vec<&str> = signature
                .parameters
                .iter()
                .map(|p| p.name.as_str())
                .collect();
            return Err(CallCommandError::Arguments(format!(
                "{service} declares no parameter {name}; it takes {}",
                if declared.is_empty() {
                    "none".to_string()
                } else {
                    declared.join(", ")
                }
            )));
        };
        let base = parameter.value.base_type.as_str();
        let fits = match base {
            _ if value.is_null() => true,
            "NUMBER" | "INTEGER" | "LONG" => value.is_number(),
            "BOOLEAN" => value.is_boolean(),
            "DATETIME" => value.is_string() || value.is_number(),
            "INFOTABLE" => value.is_object() || value.is_array(),
            "JSON" => !value.is_number() && !value.is_boolean(),
            "STRING"
            | "TEXT"
            | "HTML"
            | "HYPERLINK"
            | "IMAGELINK"
            | "PASSWORD"
            | "XML"
            | "GUID"
            | "THINGNAME"
            | "THINGTEMPLATENAME"
            | "THINGSHAPENAME"
            | "DATASHAPENAME"
            | "MASHUPNAME"
            | "MENUNAME"
            | "USERNAME"
            | "GROUPNAME"
            | "PROJECTNAME"
            | "STYLEDEFINITIONNAME"
            | "STATEDEFINITIONNAME"
            | "MEDIAENTITYNAME"
            | "LOCALIZATIONTABLENAME"
            | "ORGANIZATIONNAME"
            | "NETWORKNAME" => value.is_string(),
            _ => true,
        };
        if !fits {
            return Err(CallCommandError::Arguments(format!(
                "{service}: parameter {name} is {base}, so {value} cannot be its value"
            )));
        }
    }
    Ok(())
}

/// One call on `remote`, with the logs written during it when asked.
fn call<R: Remote>(
    request: &CallRequest,
    target: ServiceTarget,
    remote: &R,
) -> Result<CallOutcome, CallCommandError> {
    let started = logs::now_ms();
    let called = remote.call_service(
        &target,
        &request.service,
        &request.parameters,
        request.timeout,
    );
    let logged = request.with_logs.then(|| {
        logs::during_call(
            remote,
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
        read_only: None,
    })
}

/// A language preference as `SetLanguagePreferences` takes it: language tags (`de`, `pt_BR`,
/// `zh-CN`), comma-separated, the first one with a localization table winning.
fn valid_language(lang: &str) -> Result<(), CallCommandError> {
    let tag = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    };
    if lang.split(',').all(tag) {
        Ok(())
    } else {
        Err(CallCommandError::Language(format!(
            "--lang {lang:?} is not a language or a comma-separated list of them (de, pt_BR, fr,de)"
        )))
    }
}

fn user_target(user: &str) -> Result<ServiceTarget, server::ServerError> {
    ServiceTarget::entity("Users", user)
        .map_err(|why| server::ServerError::InvalidUrl(why.to_string()))
}

/// The user's stored language preference (`""` when none is set).
fn language_of<R: Remote>(
    remote: &R,
    user: &str,
    timeout: Duration,
) -> Result<String, server::ServerError> {
    let reply = remote.call_service(
        &user_target(user)?,
        "GetPropertyValues",
        &serde_json::json!({}),
        timeout,
    )?;
    Ok(reply
        .as_ref()
        .and_then(|reply| reply.pointer("/rows/0/language"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string())
}

fn set_language<R: Remote>(
    remote: &R,
    user: &str,
    preferences: &str,
    timeout: Duration,
) -> Result<(), server::ServerError> {
    remote
        .call_service(
            &user_target(user)?,
            "SetLanguagePreferences",
            &serde_json::json!({ "preferences": preferences }),
            timeout,
        )
        .map(|_| ())
}

/// The profile signed in as a declared test user. Only a `[[test.user]]` of the solution, and only
/// on a profile that allows test users: the shared test password goes to no other account.
fn as_test_user(
    solution: &Solution,
    profile: &profile::Profile,
    user: &str,
) -> Result<profile::Profile, CallCommandError> {
    use crate::core::testing::{self, TestError};
    if !solution.test.users.iter().any(|known| known.name == user) {
        return Err(CallCommandError::TestUser(TestError::Invalid(format!(
            "{user} is not a [[test.user]] in twaco.toml; --as calls only as a declared test user"
        ))));
    }
    testing::allowed(profile).map_err(CallCommandError::TestUser)?;
    testing::as_user(profile, user).map_err(CallCommandError::TestUser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn solution() -> (tempfile::TempDir, PathBuf, Solution) {
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-command-call-")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (root_guard, root, solution)
    }

    #[test]
    fn arguments_the_repository_signature_rules_out_are_refused_before_calling() {
        let (_dir, root, solution) = solution();
        std::fs::create_dir_all(root.join("Things")).unwrap();
        std::fs::write(
            root.join("Things/P.T.xml"),
            "<Entities><Things><Thing name=\"P.T\" projectName=\"P\" thingTemplate=\"GenericThing\"><ThingShape><ServiceDefinitions><ServiceDefinition name=\"Twice\"><ParameterDefinitions><FieldDefinition baseType=\"NUMBER\" name=\"x\"/><FieldDefinition aspect.isRequired=\"true\" baseType=\"STRING\" name=\"why\"/></ParameterDefinitions></ServiceDefinition></ServiceDefinitions></ThingShape></Thing></Things></Entities>",
        )
        .unwrap();
        let target = ServiceTarget::Thing("P.T".into());
        let check = |parameters: Value| check_arguments(&solution, &target, "Twice", &parameters);
        check(serde_json::json!({"x": 2, "why": "test"})).unwrap();
        let missing = check(serde_json::json!({"x": 2})).unwrap_err().to_string();
        assert!(missing.contains("requires why"), "{missing}");
        let wrong = check(serde_json::json!({"x": "abc", "why": "test"}))
            .unwrap_err()
            .to_string();
        assert!(wrong.contains("x is NUMBER"), "{wrong}");
        let unknown = check(serde_json::json!({"y": 1, "why": "test"}))
            .unwrap_err()
            .to_string();
        assert!(
            unknown.contains("no parameter y; it takes why, x"),
            "{unknown}"
        );
        // A service the repository does not know is the server's to judge.
        check_arguments(&solution, &target, "Other", &serde_json::json!({"y": 1})).unwrap();
    }

    #[test]
    fn a_plan_resolves_the_target_without_loading_a_profile_or_calling() {
        let (_dir, _, solution) = solution();
        let request = CallRequest {
            target: "Things/Outside".to_string(),
            service: "Read".to_string(),
            parameters: serde_json::json!({}),
            timeout: Duration::from_secs(1),
            mode: Mode::Plan,
            profile: "missing".to_string(),
            with_logs: false,
            profile_before_target: false,
            as_user: None,
            lang: None,
            run_if_read_only: false,
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
    }

    #[test]
    fn a_command_line_call_still_loads_its_profile_before_target_resolution() {
        let (_dir, _, solution) = solution();
        let request = CallRequest {
            target: "not a target".to_string(),
            service: "Read".to_string(),
            parameters: serde_json::json!({}),
            timeout: Duration::from_secs(1),
            mode: Mode::Apply,
            profile: "missing".to_string(),
            with_logs: false,
            profile_before_target: true,
            as_user: None,
            lang: None,
            run_if_read_only: false,
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
    }

    /// A server that records each call as `account: Collection/Name.Service body`, failing the
    /// calls whose text contains one of `fail`.
    struct Fake {
        account: String,
        log: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
        fail: Vec<&'static str>,
        /// The test user's stored language.
        previous: &'static str,
    }

    impl logs::Remote for Fake {
        fn service(
            &self,
            _: &str,
            _: &str,
            _: &Value,
        ) -> Result<Option<Value>, server::ServerError> {
            Ok(None)
        }
    }

    impl Remote for Fake {
        fn call_service(
            &self,
            target: &ServiceTarget,
            service: &str,
            parameters: &Value,
            _: Duration,
        ) -> Result<Option<Value>, server::ServerError> {
            let line = format!("{}: {target}.{service} {parameters}", self.account);
            self.log.borrow_mut().push(line.clone());
            if self.fail.iter().any(|fail| line.contains(fail)) {
                return Err(server::ServerError::Http {
                    method: server::Method::Post,
                    status: 500,
                    url: "http://server/Thingworx".into(),
                    body: "refused".into(),
                });
            }
            Ok(Some(match service {
                "GetPropertyValues" => serde_json::json!({"rows": [{"language": self.previous}]}),
                _ => serde_json::json!({"rows": [{"result": "ok"}]}),
            }))
        }
    }

    fn language_solution() -> (tempfile::TempDir, Solution) {
        let (dir, root, _) = solution();
        std::fs::write(
            root.join("twaco.toml"),
            "[[project]]\nname = \"P\"\n[[test.user]]\nname = \"vera.viewer\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join(".twaco/profiles")).unwrap();
        std::fs::write(
            root.join(".twaco/profiles/test.toml"),
            "url = \"http://server/Thingworx\"\nusername = \"Administrator\"\npassword = \"admin\"\n\
             allow_test_users = true\ntest_password = \"secret\"\n",
        )
        .unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (dir, solution)
    }

    fn language_call(
        solution: &Solution,
        as_user: Option<&str>,
        lang: &str,
        fail: Vec<&'static str>,
    ) -> (Result<CallOutcome, CallCommandError>, Vec<String>) {
        language_call_from(solution, as_user, lang, fail, "en")
    }

    fn language_call_from(
        solution: &Solution,
        as_user: Option<&str>,
        lang: &str,
        fail: Vec<&'static str>,
        previous: &'static str,
    ) -> (Result<CallOutcome, CallCommandError>, Vec<String>) {
        let request = CallRequest {
            target: "Things/Acme.Manager".to_string(),
            service: "Read".to_string(),
            parameters: serde_json::json!({}),
            timeout: Duration::from_secs(1),
            mode: Mode::Apply,
            profile: "test".to_string(),
            with_logs: false,
            profile_before_target: false,
            as_user: as_user.map(str::to_string),
            lang: Some(lang.to_string()),
            run_if_read_only: false,
        };
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let opened = log.clone();
        let outcome = execute(
            solution,
            &request,
            |profile| Fake {
                account: profile.username.clone(),
                log: opened.clone(),
                fail: fail.clone(),
                previous,
            },
            &mut Notices::default(),
        );
        let calls = log.borrow().clone();
        (outcome, calls)
    }

    #[test]
    fn lang_sets_the_test_users_language_for_the_call_and_puts_it_back() {
        let (_dir, solution) = language_solution();
        let (outcome, calls) = language_call(&solution, Some("vera.viewer"), "de", Vec::new());
        assert!(
            matches!(outcome, Ok(CallOutcome::Applied { .. })),
            "{outcome:?}"
        );
        assert_eq!(
            calls,
            [
                "Administrator: Users/vera.viewer.GetPropertyValues {}",
                r#"Administrator: Users/vera.viewer.SetLanguagePreferences {"preferences":"de"}"#,
                "vera.viewer: Things/Acme.Manager.Read {}",
                r#"Administrator: Users/vera.viewer.SetLanguagePreferences {"preferences":"en"}"#,
            ]
        );
    }

    #[test]
    fn lang_puts_the_language_back_after_a_failed_call_and_calls_nothing_when_it_cannot_set_it() {
        let (_dir, solution) = language_solution();
        let (outcome, calls) = language_call(
            &solution,
            Some("vera.viewer"),
            "de",
            vec!["Acme.Manager.Read"],
        );
        assert!(
            matches!(outcome, Err(CallCommandError::Call { .. })),
            "{outcome:?}"
        );
        assert!(
            calls.last().unwrap().ends_with(r#"{"preferences":"en"}"#),
            "{calls:?}"
        );

        let (outcome, calls) = language_call(
            &solution,
            Some("vera.viewer"),
            "de",
            vec![r#"{"preferences":"de"}"#],
        );
        assert!(
            matches!(outcome, Err(CallCommandError::SetLanguage { .. })),
            "{outcome:?}"
        );
        assert!(
            !calls.iter().any(|call| call.contains("Acme.Manager.Read")),
            "{calls:?}"
        );

        let (outcome, _) = language_call(
            &solution,
            Some("vera.viewer"),
            "de",
            vec![r#"{"preferences":"en"}"#],
        );
        let error = outcome.unwrap_err();
        assert!(
            matches!(&error, CallCommandError::RestoreLanguage { call, .. } if call.is_ok()),
            "{error:?}"
        );
        assert!(error.to_string().contains(r#"put it back with `twaco call Users/vera.viewer SetLanguagePreferences '{"preferences":"en"}'`"#), "{error}");
    }

    #[test]
    fn lang_puts_back_an_unset_preference_as_unset() {
        // A user nobody gave a language: restored with an empty preference, which 10.1 takes
        // as "none" (verified live).
        let (_dir, solution) = language_solution();
        let (outcome, calls) =
            language_call_from(&solution, Some("vera.viewer"), "de", Vec::new(), "");
        assert!(outcome.is_ok(), "{outcome:?}");
        assert_eq!(
            calls.last().unwrap(),
            r#"Administrator: Users/vera.viewer.SetLanguagePreferences {"preferences":""}"#
        );
    }

    #[test]
    fn lang_needs_as_and_a_language() {
        let (_dir, solution) = language_solution();
        let (outcome, calls) = language_call(&solution, None, "de", Vec::new());
        let error = outcome.unwrap_err();
        assert!(
            error.to_string().starts_with("--lang needs --as"),
            "{error}"
        );
        assert_eq!(error.code(), ErrorCode::InvalidArguments);
        assert!(calls.is_empty());
        for bad in ["", "de fr", "de,,fr", "de;x"] {
            let (outcome, calls) = language_call(&solution, Some("vera.viewer"), bad, Vec::new());
            assert!(
                matches!(outcome, Err(CallCommandError::Language(_))),
                "{bad:?}"
            );
            assert!(calls.is_empty());
        }
        let (outcome, _) = language_call(&solution, Some("vera.viewer"), "pt_BR,zh-CN", Vec::new());
        assert!(outcome.is_ok(), "{outcome:?}");
    }

    #[test]
    fn as_signs_in_only_as_a_declared_test_user_on_a_profile_that_allows_it() {
        let (_dir, root, _) = solution();
        std::fs::write(
            root.join("twaco.toml"),
            "[[project]]\nname = \"P\"\n[[test.user]]\nname = \"vera.viewer\"\n",
        )
        .unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let profile = |extra: &str| -> profile::Profile {
            toml::from_str(&format!(
                "url = \"http://server/Thingworx\"\nusername = \"Administrator\"\n\
                 password = \"admin\"\n{extra}"
            ))
            .unwrap()
        };
        let allowed = profile("allow_test_users = true\ntest_password = \"secret\"\n");
        let signed = as_test_user(&solution, &allowed, "vera.viewer").unwrap();
        assert_eq!(signed.username, "vera.viewer");
        assert_eq!(signed.password.expose(), "secret");
        assert!(signed.app_key.is_none());
        // Any other account would receive the test password: refused.
        let stranger = as_test_user(&solution, &allowed, "Administrator").unwrap_err();
        assert!(
            stranger.to_string().contains("not a [[test.user]]"),
            "{stranger}"
        );
        let closed = profile("test_password = \"secret\"\n");
        assert!(matches!(
            as_test_user(&solution, &closed, "vera.viewer"),
            Err(CallCommandError::TestUser(
                crate::core::testing::TestError::NotAllowed
            ))
        ));
        let no_password = profile("allow_test_users = true\n");
        assert!(matches!(
            as_test_user(&solution, &no_password, "vera.viewer"),
            Err(CallCommandError::TestUser(
                crate::core::testing::TestError::NoPassword
            ))
        ));
    }
}
