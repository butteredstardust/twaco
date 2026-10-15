//! `twaco users list|seed|delete`: the test users of `[[test.user]]` on the server.
//!
//! `seed` creates what is missing (the `[test] project`, each user, each group membership) and
//! never changes what exists: an existing user's password is not reset, and a user that cannot
//! sign in with the profile's `test_password` is reported. A group must already exist: it is the
//! solution's, deployed with it, never a test fixture. `delete` removes the users only.

use super::{allowed, test_password, Remote, TestError};
use crate::core::config::Solution;
use crate::core::entity_key::{EntityKey, ServiceTarget};
use crate::core::profile::Profile;
use crate::core::server::ServerError;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// What a user is on the server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserState {
    pub name: String,
    pub exists: bool,
    /// Whether the user signs in with the profile's test password; `None` when the user does
    /// not exist or the profile has no test password.
    pub signs_in: Option<bool>,
    pub groups: Vec<(String, Membership)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Membership {
    Member,
    Missing,
    /// The group itself is not on the server.
    NoGroup,
}

/// One server change a seed or a delete makes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    CreateProject(String),
    CreateUser(String),
    AddMember { user: String, group: String },
    DeleteUser(String),
}

impl std::fmt::Display for Step {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CreateProject(name) => write!(f, "create project {name}"),
            Self::CreateUser(name) => write!(f, "create user {name}"),
            Self::AddMember { user, group } => write!(f, "add {user} to {group}"),
            Self::DeleteUser(name) => write!(f, "delete user {name}"),
        }
    }
}

/// What a seed or a delete did, or would do.
#[derive(Debug)]
pub struct Changed {
    pub steps: Vec<Step>,
    pub applied: bool,
    /// The users afterwards (as they are, for a plan).
    pub users: Vec<UserState>,
}

fn entity_services() -> ServiceTarget {
    ServiceTarget::platform("Resources", "EntityServices")
}

fn key(collection: &str, name: &str) -> Result<EntityKey, TestError> {
    EntityKey::new(collection, name).map_err(|error| TestError::Invalid(error.to_string()))
}

/// Every declared user as the server has it. `sign_in` opens a session as one user; it is only
/// asked for users that exist, and only when the profile has a test password.
pub fn list<R, S, F>(
    solution: &Solution,
    profile: &Profile,
    admin: &R,
    sign_in: F,
) -> Result<Vec<UserState>, TestError>
where
    R: Remote + ?Sized,
    S: Remote,
    F: Fn(&str) -> Result<S, TestError>,
{
    if solution.test.users.is_empty() {
        return Err(TestError::NoUsers);
    }
    let mut members: BTreeMap<&str, Option<Vec<String>>> = BTreeMap::new();
    let mut out = Vec::new();
    for user in &solution.test.users {
        let exists = admin.exists(&key("Users", &user.name)?)?;
        let mut groups = Vec::new();
        for group in &user.groups {
            if !members.contains_key(group.as_str()) {
                let found = group_members(admin, group)?;
                members.insert(group, found);
            }
            let state = match &members[group.as_str()] {
                None => Membership::NoGroup,
                Some(names) if names.contains(&user.name) => Membership::Member,
                Some(_) => Membership::Missing,
            };
            groups.push((group.clone(), state));
        }
        let signs_in = if exists && test_password(profile).is_ok() {
            Some(signs_in(&sign_in(&user.name)?, &user.name)?)
        } else {
            None
        };
        out.push(UserState {
            name: user.name.clone(),
            exists,
            signs_in,
            groups,
        });
    }
    Ok(out)
}

/// The user names in a group, or `None` when the group is not on the server.
fn group_members<R: Remote + ?Sized>(
    admin: &R,
    group: &str,
) -> Result<Option<Vec<String>>, TestError> {
    let group_key = key("Groups", group)?;
    if !admin.exists(&group_key)? {
        return Ok(None);
    }
    let reply = admin.call(
        &ServiceTarget::Entity(group_key),
        "GetGroupMembers",
        &json!({}),
    )?;
    Ok(Some(
        reply
            .as_ref()
            .and_then(|value| value.get("rows"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|row| row.get("type").and_then(Value::as_str) != Some("Group"))
            .filter_map(|row| row.get("name").and_then(Value::as_str).map(str::to_string))
            .collect(),
    ))
}

/// Whether a session as `user` is the user: the server answers who is signed in. A user with no
/// permission on `CurrentSessionInfo` (a new account in no group) is answered 404, or 403, and
/// is signed in all the same; only a 401 says the name or the password is wrong.
fn signs_in<S: Remote>(session: &S, user: &str) -> Result<bool, TestError> {
    let reply = session.call(
        &ServiceTarget::platform("Resources", "CurrentSessionInfo"),
        "GetCurrentUser",
        &json!({}),
    );
    match reply {
        Ok(reply) => Ok(reply
            .as_ref()
            .and_then(|value| value.get("rows"))
            .and_then(Value::as_array)
            .and_then(|rows| rows.first())
            .and_then(|row| row.get("result"))
            .and_then(Value::as_str)
            == Some(user)),
        Err(ServerError::Http { status: 401, .. }) => Ok(false),
        Err(ServerError::Http {
            status: 403 | 404, ..
        }) => Ok(true),
        Err(error) => Err(error.into()),
    }
}

/// The users as JSON, the same for the command line and MCP.
pub fn users_json(users: &[UserState]) -> Value {
    json!(users
        .iter()
        .map(|user| json!({
            "name": user.name,
            "exists": user.exists,
            "signs_in": user.signs_in,
            "groups": user.groups.iter().map(|(group, state)| json!({
                "group": group,
                "state": match state {
                    Membership::Member => "member",
                    Membership::Missing => "missing",
                    Membership::NoGroup => "no group",
                },
            })).collect::<Vec<_>>(),
        }))
        .collect::<Vec<_>>())
}

/// Create what is missing. A group that is not on the server is refused before anything is
/// sent: deploy the solution first.
pub fn seed<R, S, F>(
    solution: &Solution,
    profile: &Profile,
    admin: &R,
    sign_in: F,
    apply: bool,
) -> Result<Changed, TestError>
where
    R: Remote + ?Sized,
    S: Remote,
    F: Fn(&str) -> Result<S, TestError>,
{
    allowed(profile)?;
    let password = test_password(profile)?.to_string();
    let before = list(solution, profile, admin, &sign_in)?;
    let missing_groups: Vec<String> = before
        .iter()
        .flat_map(|user| user.groups.iter())
        .filter(|(_, state)| *state == Membership::NoGroup)
        .map(|(group, _)| group.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    if !missing_groups.is_empty() {
        return Err(TestError::Invalid(format!(
            "group(s) {} are not on the server; deploy the solution first, nothing was created",
            missing_groups.join(", ")
        )));
    }
    let mut steps = Vec::new();
    if let Some(project) = &solution.test.project {
        if !admin.exists(&key("Projects", project)?)? {
            steps.push(Step::CreateProject(project.clone()));
        }
    }
    for user in &before {
        if !user.exists {
            steps.push(Step::CreateUser(user.name.clone()));
        }
        for (group, state) in &user.groups {
            if *state == Membership::Missing {
                steps.push(Step::AddMember {
                    user: user.name.clone(),
                    group: group.clone(),
                });
            }
        }
    }
    if !apply {
        return Ok(Changed {
            steps,
            applied: false,
            users: before,
        });
    }
    for step in &steps {
        match step {
            Step::CreateProject(name) => {
                admin.call(
                    &entity_services(),
                    "CreateProject",
                    &json!({
                        "name": name,
                        "type": "Component",
                        "dependsOn": "",
                        "tags": "",
                        "description": "Test users created by twaco for `twaco test`.",
                    }),
                )?;
            }
            Step::CreateUser(name) => {
                admin.call(
                    &entity_services(),
                    "CreateUser",
                    &json!({
                        "name": name,
                        "password": password,
                        "description": "Test user created by twaco for `twaco test`.",
                        "tags": "",
                        "projectName": solution.test.project.clone().unwrap_or_default(),
                    }),
                )?;
            }
            // `member`, not `name`; adding someone already in the group is not an error.
            Step::AddMember { user, group } => {
                admin.call(
                    &ServiceTarget::Entity(key("Groups", group)?),
                    "AddMember",
                    &json!({ "member": user, "type": "User" }),
                )?;
            }
            Step::DeleteUser(_) => unreachable!("a seed deletes nothing"),
        }
    }
    let users = list(solution, profile, admin, &sign_in)?;
    Ok(Changed {
        steps,
        applied: true,
        users,
    })
}

/// Delete every declared user the server has. Groups and the project stay: they may hold more.
pub fn delete<R: Remote + ?Sized>(
    solution: &Solution,
    profile: &Profile,
    admin: &R,
    apply: bool,
) -> Result<Changed, TestError> {
    allowed(profile)?;
    if solution.test.users.is_empty() {
        return Err(TestError::NoUsers);
    }
    let mut steps = Vec::new();
    for user in &solution.test.users {
        if admin.exists(&key("Users", &user.name)?)? {
            steps.push(Step::DeleteUser(user.name.clone()));
        }
    }
    if apply {
        for step in &steps {
            if let Step::DeleteUser(name) = step {
                admin.call(&entity_services(), "DeleteUser", &json!({ "name": name }))?;
            }
        }
    }
    Ok(Changed {
        steps,
        applied: apply,
        users: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::server::Method;
    use std::sync::Mutex;

    /// A server holding users, groups and members, and the one password every user has.
    #[derive(Default)]
    struct Server {
        users: Mutex<Vec<String>>,
        groups: BTreeMap<String, Mutex<Vec<String>>>,
        projects: Mutex<Vec<String>>,
        password: String,
        calls: Mutex<Vec<String>>,
    }

    /// A session on `Server`: the administrator, or a user with a password.
    struct Session<'a> {
        server: &'a Server,
        user: Option<(String, String)>,
    }

    impl Remote for Session<'_> {
        fn exists(&self, key: &EntityKey) -> Result<bool, ServerError> {
            let name = key.name().to_string();
            Ok(match key.collection() {
                "Users" => self.server.users.lock().unwrap().contains(&name),
                "Groups" => self.server.groups.contains_key(&name),
                "Projects" => self.server.projects.lock().unwrap().contains(&name),
                other => panic!("{other}"),
            })
        }

        fn call(
            &self,
            target: &ServiceTarget,
            service: &str,
            parameters: &Value,
        ) -> Result<Option<Value>, ServerError> {
            if let Some((user, password)) = &self.user {
                let known = self.server.users.lock().unwrap().contains(user);
                if !known || *password != self.server.password {
                    return Err(ServerError::Http {
                        method: Method::Post,
                        status: 401,
                        url: String::new(),
                        body: String::new(),
                    });
                }
                assert_eq!(service, "GetCurrentUser");
                // As observed live: a user in no group may not see CurrentSessionInfo.
                let in_a_group = self
                    .server
                    .groups
                    .values()
                    .any(|members| members.lock().unwrap().contains(user));
                if !in_a_group {
                    return Err(ServerError::Http {
                        method: Method::Post,
                        status: 404,
                        url: String::new(),
                        body: String::new(),
                    });
                }
                return Ok(Some(json!({ "rows": [{ "result": user }] })));
            }
            self.server
                .calls
                .lock()
                .unwrap()
                .push(format!("{target} {service}"));
            let field = |name: &str| parameters[name].as_str().unwrap().to_string();
            let group = || target.to_string().trim_start_matches("Groups/").to_string();
            match service {
                "GetGroupMembers" => {
                    let rows: Vec<Value> = self.server.groups[&group()]
                        .lock()
                        .unwrap()
                        .iter()
                        .map(|name| json!({ "name": name, "type": "User" }))
                        .collect();
                    Ok(Some(json!({ "rows": rows })))
                }
                "CreateUser" => {
                    assert_eq!(field("password"), self.server.password);
                    assert!(!field("description").contains(&self.server.password));
                    self.server.users.lock().unwrap().push(field("name"));
                    Ok(None)
                }
                "CreateProject" => {
                    self.server.projects.lock().unwrap().push(field("name"));
                    Ok(None)
                }
                "AddMember" => {
                    self.server.groups[&group()]
                        .lock()
                        .unwrap()
                        .push(field("member"));
                    Ok(None)
                }
                "DeleteUser" => {
                    let name = field("name");
                    self.server
                        .users
                        .lock()
                        .unwrap()
                        .retain(|user| *user != name);
                    Ok(None)
                }
                other => panic!("{other}"),
            }
        }
    }

    fn solution(toml: &str) -> (tempfile::TempDir, Solution) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("twaco.toml"), toml).unwrap();
        let solution = Solution::load(&dir.path().join("twaco.toml")).unwrap();
        (dir, solution)
    }

    const TOML: &str =
        "[[project]]\nname = \"Acme.App\"\n\n[test]\nproject = \"Acme.App.TestUsers\"\n\n\
[[test.user]]\nname = \"vera.viewer\"\ngroups = [\"Acme.App.Viewers_UG\"]\n\n\
[[test.user]]\nname = \"ed.editor\"\ngroups = [\"Acme.App.Viewers_UG\", \"Acme.App.Editors_UG\"]\n\n\
[[test.user]]\nname = \"nora.none\"\n";

    fn profile(extra: &str) -> Profile {
        toml::from_str(&format!(
            "url = \"http://server/Thingworx\"\nusername = \"Administrator\"\npassword = \"admin\"\n{extra}"
        ))
        .unwrap()
    }

    fn server() -> Server {
        Server {
            groups: [
                (
                    "Acme.App.Viewers_UG".to_string(),
                    Mutex::new(vec!["ed.editor".to_string()]),
                ),
                ("Acme.App.Editors_UG".to_string(), Mutex::new(Vec::new())),
            ]
            .into_iter()
            .collect(),
            users: Mutex::new(vec!["ed.editor".to_string()]),
            password: "secret".to_string(),
            ..Server::default()
        }
    }

    fn sign_in<'a>(
        server: &'a Server,
        profile: &'a Profile,
    ) -> impl Fn(&str) -> Result<Session<'a>, TestError> {
        move |user| {
            Ok(Session {
                server,
                user: Some((user.to_string(), test_password(profile)?.to_string())),
            })
        }
    }

    fn only_reads(server: &Server) -> bool {
        server
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|call| call.ends_with("GetGroupMembers"))
    }

    #[test]
    fn a_seed_plans_what_is_missing_then_creates_it_and_every_user_signs_in() {
        let (_dir, solution) = solution(TOML);
        let server = server();
        let admin = Session {
            server: &server,
            user: None,
        };
        let profile = profile("allow_test_users = true\ntest_password = \"secret\"\n");
        let plan = seed(
            &solution,
            &profile,
            &admin,
            sign_in(&server, &profile),
            false,
        )
        .unwrap();
        let steps: Vec<String> = plan.steps.iter().map(ToString::to_string).collect();
        assert_eq!(
            steps,
            [
                "create project Acme.App.TestUsers",
                "create user vera.viewer",
                "add vera.viewer to Acme.App.Viewers_UG",
                "add ed.editor to Acme.App.Editors_UG",
                "create user nora.none",
            ]
        );
        assert!(!plan.applied);
        assert!(only_reads(&server), "a plan sends nothing");
        let seeded = seed(
            &solution,
            &profile,
            &admin,
            sign_in(&server, &profile),
            true,
        )
        .unwrap();
        assert!(seeded.applied);
        assert!(seeded.users.iter().all(|user| user.exists
            && user.signs_in == Some(true)
            && user
                .groups
                .iter()
                .all(|(_, state)| *state == Membership::Member)));
        server.calls.lock().unwrap().clear();
        let again = seed(
            &solution,
            &profile,
            &admin,
            sign_in(&server, &profile),
            true,
        )
        .unwrap();
        assert!(again.steps.is_empty());
        assert!(only_reads(&server), "a second seed sends nothing");
    }

    #[test]
    fn a_user_that_cannot_sign_in_with_the_test_password_is_reported_not_reset() {
        let (_dir, solution) = solution(TOML);
        let server = Server {
            password: "another".to_string(),
            ..server()
        };
        let admin = Session {
            server: &server,
            user: None,
        };
        let profile = profile("test_password = \"secret\"\n");
        let users = list(&solution, &profile, &admin, sign_in(&server, &profile)).unwrap();
        let ed = users.iter().find(|user| user.name == "ed.editor").unwrap();
        assert_eq!(ed.signs_in, Some(false));
        let vera = users
            .iter()
            .find(|user| user.name == "vera.viewer")
            .unwrap();
        assert_eq!((vera.exists, vera.signs_in), (false, None));
    }

    #[test]
    fn writes_need_a_profile_that_allows_test_users_and_a_test_password() {
        let (_dir, solution) = solution(TOML);
        let server = server();
        let admin = Session {
            server: &server,
            user: None,
        };
        let none = profile("test_password = \"secret\"\n");
        assert!(matches!(
            seed(&solution, &none, &admin, sign_in(&server, &none), false),
            Err(TestError::NotAllowed)
        ));
        assert!(matches!(
            delete(&solution, &none, &admin, false),
            Err(TestError::NotAllowed)
        ));
        let no_password = profile("allow_test_users = true\n");
        let error = seed(
            &solution,
            &no_password,
            &admin,
            sign_in(&server, &no_password),
            false,
        )
        .unwrap_err();
        assert!(matches!(error, TestError::NoPassword));
        assert!(server.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn a_group_missing_from_the_server_refuses_the_seed_before_anything_is_created() {
        let (_dir, solution) = solution(&TOML.replace("Acme.App.Editors_UG", "Acme.App.Gone_UG"));
        let server = server();
        let admin = Session {
            server: &server,
            user: None,
        };
        let profile = profile("allow_test_users = true\ntest_password = \"secret\"\n");
        let error = seed(
            &solution,
            &profile,
            &admin,
            sign_in(&server, &profile),
            true,
        )
        .unwrap_err();
        assert!(error.to_string().contains("Acme.App.Gone_UG"), "{error}");
        assert!(only_reads(&server));
    }

    #[test]
    fn delete_removes_the_users_the_server_has_and_nothing_else() {
        let (_dir, solution) = solution(TOML);
        let server = server();
        let admin = Session {
            server: &server,
            user: None,
        };
        let profile = profile("allow_test_users = true\n");
        let plan = delete(&solution, &profile, &admin, false).unwrap();
        assert_eq!(plan.steps, [Step::DeleteUser("ed.editor".to_string())]);
        assert!(
            server.calls.lock().unwrap().is_empty(),
            "a plan sends nothing"
        );
        delete(&solution, &profile, &admin, true).unwrap();
        assert!(server.users.lock().unwrap().is_empty());
        let (_dir, empty) = super::tests::solution("[[project]]\nname = \"Acme.App\"\n");
        assert!(matches!(
            delete(&empty, &profile, &admin, false),
            Err(TestError::NoUsers)
        ));
    }
}
