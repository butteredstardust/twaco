//! `twaco test`: service cases from `tests/*.toml`, each called as one or more test users, each
//! expecting a status (200, 401, 404, ...) and, for a reply, optionally its row count and first
//! row.
//!
//! ```toml
//! [[case]]
//! name = "a viewer reads the dashboards"
//! service = "Acme.App.Manager.GetDashboards"   # Thing.Service, or Collection/Name.Service
//! as = ["vera.viewer", "Acme.App.Editors_UG"]  # test users, or groups: every test user in one
//! expect = 200
//! params = { limit = 10 }
//! rows = 2                                      # optional, with expect = 200
//! first = { name = "Overview" }                 # optional: these fields of the first row
//! ```
//!
//! A case calls the service as written: a case that creates something creates it. The cases run
//! one after another, in file and case order.

use super::{Remote, TestError};
use crate::core::config::Solution;
use crate::core::entity_key::ServiceTarget;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// The default folder of case files.
pub const DEFAULT_FOLDER: &str = "tests";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default, rename = "case")]
    cases: Vec<RawCase>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCase {
    name: Option<String>,
    service: String,
    #[serde(rename = "as")]
    users: Vec<String>,
    expect: u16,
    #[serde(default)]
    params: toml::Table,
    rows: Option<usize>,
    first: Option<toml::Table>,
}

/// One case, checked and ready to run.
#[derive(Clone, Debug)]
pub struct Case {
    pub file: PathBuf,
    pub name: String,
    pub target: ServiceTarget,
    pub service: String,
    /// The test users it runs as, groups expanded, in order and without repeats.
    pub users: Vec<String>,
    pub expect: u16,
    pub params: Value,
    pub rows: Option<usize>,
    pub first: Option<Map<String, Value>>,
}

/// One call of a case as one user.
#[derive(Clone, Debug)]
pub struct Outcome {
    /// The case's position in the list run.
    pub case: usize,
    pub user: String,
    /// The status the server answered with; `None` when it answered with no status at all.
    pub status: Option<u16>,
    /// Why the call did not do what the case expects; `None` when it passed.
    pub failure: Option<String>,
}

/// The folder of case files.
pub fn folder(solution: &Solution) -> PathBuf {
    solution
        .root
        .join(solution.test.cases.as_deref().unwrap_or(DEFAULT_FOLDER))
}

/// Read every case file (`*.toml` directly in the folder, in name order), keeping the cases whose
/// name, service or file contains one of `filters` (all of them without filters). Every problem
/// in every file is reported at once, and nothing runs while there is one.
pub fn load(solution: &Solution, filters: &[String]) -> Result<Vec<Case>, TestError> {
    super::check_users(solution)?;
    let folder = folder(solution);
    let mut paths: Vec<PathBuf> = match std::fs::read_dir(&folder) {
        Ok(entries) => entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("toml"))
            })
            .collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => {
            return Err(TestError::Invalid(format!("{}: {error}", folder.display())));
        }
    };
    paths.sort();
    if paths.is_empty() {
        return Err(TestError::Invalid(format!(
            "no case files (*.toml) in {}",
            folder.display()
        )));
    }
    let mut problems = Vec::new();
    let mut cases = Vec::new();
    for path in &paths {
        let shown = path
            .strip_prefix(&solution.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                problems.push(format!("{shown}: {error}"));
                continue;
            }
        };
        let file: File = match toml::from_str(&text) {
            Ok(file) => file,
            Err(error) => {
                problems.push(format!("{shown}: {}", error.message()));
                continue;
            }
        };
        if file.cases.is_empty() {
            problems.push(format!("{shown}: holds no [[case]]"));
        }
        for (index, raw) in file.cases.into_iter().enumerate() {
            let name = raw
                .name
                .clone()
                .unwrap_or_else(|| format!("{} #{}", raw.service, index + 1));
            match case(solution, path, &name, raw) {
                Ok(case) => cases.push(case),
                Err(why) => problems.push(format!("{shown}: case {name}: {why}")),
            }
        }
    }
    if !problems.is_empty() {
        return Err(TestError::Invalid(problems.join("\n")));
    }
    cases.retain(|case| {
        filters.is_empty()
            || filters.iter().any(|filter| {
                case.name.contains(filter.as_str())
                    || case.service.contains(filter.as_str())
                    || case.target.to_string().contains(filter.as_str())
                    || case.file.to_string_lossy().contains(filter.as_str())
            })
    });
    if cases.is_empty() {
        return Err(TestError::Invalid(format!(
            "no case matches {}",
            filters.join(", ")
        )));
    }
    Ok(cases)
}

fn case(solution: &Solution, path: &Path, name: &str, raw: RawCase) -> Result<Case, String> {
    let (target, service) = raw
        .service
        .rsplit_once('.')
        .filter(|(target, service)| !target.is_empty() && !service.is_empty())
        .ok_or_else(|| {
            format!(
                "service {} is not Thing.Service or Collection/Name.Service",
                raw.service
            )
        })?;
    let target = ServiceTarget::parse(target).map_err(|error| error.to_string())?;
    // A reply is 200 whatever 2xx the server sent: twaco cannot tell them apart.
    if raw.expect != 200 && !(400..=599).contains(&raw.expect) {
        return Err(format!(
            "expect {} is not 200 or a refusal (400 to 599)",
            raw.expect
        ));
    }
    if raw.expect != 200 && (raw.rows.is_some() || raw.first.is_some()) {
        return Err("rows and first check a reply: they need expect = 200".to_string());
    }
    if raw.users.is_empty() {
        return Err("as names no test user".to_string());
    }
    let mut users: Vec<String> = Vec::new();
    for named in &raw.users {
        let found: Vec<&str> = if solution.test.users.iter().any(|user| user.name == *named) {
            vec![named.as_str()]
        } else {
            solution
                .test
                .users
                .iter()
                .filter(|user| user.groups.contains(named))
                .map(|user| user.name.as_str())
                .collect()
        };
        if found.is_empty() {
            return Err(format!(
                "{named} is neither a [[test.user]] nor a group one of them is in"
            ));
        }
        for user in found {
            if !users.iter().any(|known| known == user) {
                users.push(user.to_string());
            }
        }
    }
    let json = |table: toml::Table| serde_json::to_value(table).map_err(|error| error.to_string());
    let first = match raw.first {
        Some(table) => match json(table)? {
            Value::Object(map) => Some(map),
            _ => unreachable!("a table is an object"),
        },
        None => None,
    };
    Ok(Case {
        file: path.to_path_buf(),
        name: name.to_string(),
        target,
        service: service.to_string(),
        users,
        expect: raw.expect,
        params: json(raw.params)?,
        rows: raw.rows,
        first,
    })
}

/// Run every case as each of its users, one call after another. `sign_in` opens a session as
/// a test user.
pub fn run<S, F>(cases: &[Case], sign_in: F) -> Result<Vec<Outcome>, TestError>
where
    S: Remote,
    F: Fn(&str) -> Result<S, TestError>,
{
    let mut out = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        for user in &case.users {
            let session = sign_in(user)?;
            let reply = session.call(&case.target, &case.service, &case.params);
            let status = super::status(&reply);
            let failure = match (&reply, status) {
                (Err(error), None) => Some(format!("no answer: {error}")),
                (_, Some(status)) if status != case.expect => {
                    Some(format!("answered {status}, expected {}", case.expect))
                }
                (Ok(reply), _) => check_reply(case, reply.as_ref()),
                _ => None,
            };
            out.push(Outcome {
                case: index,
                user: user.clone(),
                status,
                failure,
            });
        }
    }
    Ok(out)
}

/// The outcomes as JSON, the same for the command line and MCP: counts first, then each call.
pub fn outcomes_json(cases: &[Case], outcomes: &[Outcome]) -> Value {
    let failed = outcomes
        .iter()
        .filter(|outcome| outcome.failure.is_some())
        .count();
    serde_json::json!({
        "passed": outcomes.len() - failed,
        "failed": failed,
        "calls": outcomes.iter().map(|outcome| {
            let case = &cases[outcome.case];
            serde_json::json!({
                "case": case.name,
                "service": format!("{}.{}", case.target, case.service),
                "user": outcome.user,
                "expected": case.expect,
                "status": outcome.status,
                "failure": outcome.failure,
            })
        }).collect::<Vec<_>>(),
    })
}

/// The cases a dry run would call, as JSON: the same for the command line and MCP.
pub fn plan_json(cases: &[Case]) -> Value {
    serde_json::json!({
        "dry_run": true,
        "cases": cases.iter().map(|case| serde_json::json!({
            "case": case.name,
            "service": format!("{}.{}", case.target, case.service),
            "as": case.users,
            "expect": case.expect,
        })).collect::<Vec<_>>(),
    })
}

/// What a reply lacks of what the case expects of it.
fn check_reply(case: &Case, reply: Option<&Value>) -> Option<String> {
    if case.rows.is_none() && case.first.is_none() {
        return None;
    }
    let rows = reply
        .and_then(|value| value.get("rows"))
        .and_then(Value::as_array);
    let Some(rows) = rows else {
        return Some("the reply is not an InfoTable".to_string());
    };
    if let Some(expected) = case.rows {
        if rows.len() != expected {
            return Some(format!("{} row(s), expected {expected}", rows.len()));
        }
    }
    if let Some(first) = &case.first {
        let Some(row) = rows.first() else {
            return Some("no first row".to_string());
        };
        for (field, expected) in first {
            let actual = row.get(field).unwrap_or(&Value::Null);
            if !same(actual, expected) {
                return Some(format!(
                    "first row's {field} is {actual}, expected {expected}"
                ));
            }
        }
    }
    None
}

/// JSON equality, numbers compared by value: ThingWorx answers `3.0` for a NUMBER a case writes
/// as `3`.
fn same(actual: &Value, expected: &Value) -> bool {
    match (actual.as_f64(), expected.as_f64()) {
        (Some(actual), Some(expected)) => actual == expected,
        _ => actual == expected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::entity_key::EntityKey;
    use crate::core::server::{Method, ServerError};
    use serde_json::json;

    const TOML: &str = "[[project]]\nname = \"Acme.App\"\n\n\
[[test.user]]\nname = \"vera.viewer\"\ngroups = [\"Acme.App.Viewers_UG\"]\n\n\
[[test.user]]\nname = \"ed.editor\"\ngroups = [\"Acme.App.Viewers_UG\", \"Acme.App.Editors_UG\"]\n\n\
[[test.user]]\nname = \"nobody\"\n";

    fn solution(files: &[(&str, &str)]) -> (tempfile::TempDir, Solution) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("twaco.toml"), TOML).unwrap();
        for (name, text) in files {
            let path = dir.path().join("tests").join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let solution = Solution::load(&dir.path().join("twaco.toml")).unwrap();
        (dir, solution)
    }

    const CASES: &str = "[[case]]\nname = \"read\"\nservice = \"Acme.App.Manager.GetDashboards\"\n\
as = [\"Acme.App.Viewers_UG\", \"nobody\"]\nexpect = 200\nparams = { limit = 3 }\nrows = 1\n\
first = { name = \"Overview\", count = 3 }\n\n\
[[case]]\nname = \"edit\"\nservice = \"Acme.App.Manager.SaveDashboard\"\nas = [\"vera.viewer\"]\nexpect = 401\n";

    #[test]
    fn cases_are_read_with_groups_expanded_to_their_users() {
        let (_dir, solution) = solution(&[("dashboards.toml", CASES)]);
        let cases = load(&solution, &[]).unwrap();
        assert_eq!(cases.len(), 2);
        assert_eq!(cases[0].target.to_string(), "Acme.App.Manager");
        assert_eq!(cases[0].service, "GetDashboards");
        assert_eq!(cases[0].users, ["vera.viewer", "ed.editor", "nobody"]);
        assert_eq!(cases[0].params, json!({ "limit": 3 }));
        let only = load(&solution, &["edit".to_string()]).unwrap();
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].name, "edit");
    }

    #[test]
    fn every_problem_in_every_file_is_reported_and_nothing_loads() {
        let (_dir, solution) = solution(&[
            (
                "a.toml",
                "[[case]]\nservice = \"NoDot\"\nas = [\"vera.viewer\"]\nexpect = 200\n",
            ),
            (
                "b.toml",
                "[[case]]\nservice = \"T.S\"\nas = [\"stranger\"]\nexpect = 200\n",
            ),
            (
                "c.toml",
                "[[case]]\nservice = \"T.S\"\nas = [\"vera.viewer\"]\nexpect = 401\nrows = 1\n",
            ),
            (
                "d.toml",
                "[[case]]\nservice = \"T.S\"\nas = [\"vera.viewer\"]\nexpect = 200\ntypo = 1\n",
            ),
            ("e.toml", "# nothing here\n"),
            (
                "f.toml",
                "[[case]]\nservice = \"T.S\"\nas = [\"vera.viewer\"]\nexpect = 204\n",
            ),
        ]);
        let error = load(&solution, &[]).unwrap_err().to_string();
        for expected in [
            "tests/a.toml: case NoDot #1: service NoDot is not",
            "tests/b.toml: case T.S #1: stranger is neither",
            "tests/c.toml: case T.S #1: rows and first check a reply",
            "tests/d.toml: unknown field `typo`",
            "tests/e.toml: holds no [[case]]",
            "tests/f.toml: case T.S #1: expect 204 is not 200 or a refusal",
        ] {
            assert!(error.contains(expected), "{expected} in {error}");
        }
        let (_dir, empty) = super::tests::solution(&[]);
        assert!(load(&empty, &[])
            .unwrap_err()
            .to_string()
            .contains("no case files"));
    }

    /// A server where viewers read, nobody is in no role, and only editors save.
    struct Session(String);

    impl Remote for Session {
        fn exists(&self, _: &EntityKey) -> Result<bool, ServerError> {
            unreachable!()
        }

        fn call(
            &self,
            target: &ServiceTarget,
            service: &str,
            parameters: &Value,
        ) -> Result<Option<Value>, ServerError> {
            assert_eq!(target.to_string(), "Acme.App.Manager");
            let refuse = |status| {
                Err(ServerError::Http {
                    method: Method::Post,
                    status,
                    url: String::new(),
                    body: String::new(),
                })
            };
            match (self.0.as_str(), service) {
                ("nobody", _) => refuse(404),
                ("vera.viewer", "SaveDashboard") => refuse(401),
                (_, "GetDashboards") => {
                    assert_eq!(parameters["limit"], 3);
                    let rows = if self.0 == "ed.editor" {
                        json!([{ "name": "Overview", "count": 3.0 }, { "name": "Extra", "count": 1.0 }])
                    } else {
                        json!([{ "name": "Overview", "count": 3.0 }])
                    };
                    Ok(Some(json!({ "rows": rows })))
                }
                _ => Ok(None),
            }
        }
    }

    #[test]
    fn each_case_runs_as_each_user_and_says_what_differed() {
        let (_dir, solution) = solution(&[("dashboards.toml", CASES)]);
        let cases = load(&solution, &[]).unwrap();
        let outcomes = run(&cases, |user| Ok(Session(user.to_string()))).unwrap();
        let seen: Vec<(String, Option<u16>, Option<String>)> = outcomes
            .into_iter()
            .map(|outcome| (outcome.user, outcome.status, outcome.failure))
            .collect();
        assert_eq!(
            seen,
            [
                ("vera.viewer".to_string(), Some(200), None),
                (
                    "ed.editor".to_string(),
                    Some(200),
                    Some("2 row(s), expected 1".to_string())
                ),
                (
                    "nobody".to_string(),
                    Some(404),
                    Some("answered 404, expected 200".to_string())
                ),
                ("vera.viewer".to_string(), Some(401), None),
            ]
        );
    }

    #[test]
    fn a_first_row_is_compared_field_by_field_numbers_by_value() {
        let case = |first: Value| Case {
            file: PathBuf::new(),
            name: "c".to_string(),
            target: ServiceTarget::Thing("T".to_string()),
            service: "S".to_string(),
            users: Vec::new(),
            expect: 200,
            params: json!({}),
            rows: None,
            first: first.as_object().cloned(),
        };
        let reply = json!({ "rows": [{ "name": "a", "count": 3.0, "on": true }] });
        assert_eq!(
            check_reply(&case(json!({ "count": 3, "on": true })), Some(&reply)),
            None
        );
        assert_eq!(
            check_reply(&case(json!({ "name": "b" })), Some(&reply)).unwrap(),
            "first row's name is \"a\", expected \"b\""
        );
        assert_eq!(
            check_reply(&case(json!({ "name": "a" })), None).unwrap(),
            "the reply is not an InfoTable"
        );
    }
}
