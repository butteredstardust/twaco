//! The server half of the audit, against a fake server.

use super::audit::{self, Severity};
use crate::core::config::Solution;
use crate::core::entity_carry::{self, Kind};
use crate::core::entity_key::{EntityKey, ServiceTarget};
use crate::core::server::ServerError;
use crate::core::{config_table, push};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// What the fake server holds: permission sets by entity and set, group members, exports.
#[derive(Default)]
pub(super) struct Fake {
    pub(super) sets: BTreeMap<(String, &'static str), Value>,
    /// Configuration tables by name, as GetConfigurationTable answers.
    pub(super) tables: BTreeMap<String, Value>,
    /// Entities the server does not have.
    pub(super) missing: Vec<String>,
    /// What AddMember and AddRunTimePermission added: (entity, parameters).
    added: Mutex<Vec<(String, Value)>>,
    members: BTreeMap<String, Vec<String>>,
    exports: BTreeMap<String, String>,
    projects: Vec<String>,
    calls: Mutex<Vec<String>>,
}

fn empty(kind: Kind) -> Value {
    if kind.is_run_time() {
        json!({"permissions": []})
    } else if kind.is_design_time() {
        json!({"Create": [], "Read": [], "Update": [], "Delete": [], "Metadata": []})
    } else {
        json!({"Visibility": []})
    }
}

impl entity_carry::Remote for Fake {
    fn exists(&self, collection: &str, name: &str) -> Result<bool, ServerError> {
        if self.missing.iter().any(|m| m == name) {
            return Ok(false);
        }
        Ok(collection != "Projects" || self.projects.iter().any(|p| p == name))
    }
    fn get(&self, _: &str, name: &str, kind: Kind) -> Result<Value, ServerError> {
        let mut value = self
            .sets
            .get(&(name.to_string(), kind.label()))
            .cloned()
            .unwrap_or_else(|| empty(kind));
        for (entity, added) in self.added.lock().unwrap().iter() {
            if entity == name && added.get("resource").is_some() {
                value["permissions"].as_array_mut().unwrap().push(json!({
                    "resourceName": added["resource"],
                    added["type"].as_str().unwrap(): [
                        {"isPermitted": true, "name": added["principal"], "type": "Group"}]
                }));
            }
        }
        Ok(value)
    }
    fn set(&self, _: &str, _: &str, _: Kind, _: &Value) -> Result<(), ServerError> {
        self.calls.lock().unwrap().push("set".to_string());
        Ok(())
    }
    fn differences(&self, _: &str, _: &str, _: &str) -> Result<usize, ServerError> {
        Ok(0)
    }
}

impl config_table::Remote for Fake {
    fn call(
        &self,
        target: &ServiceTarget,
        service: &str,
        parameters: &Value,
    ) -> Result<Option<Value>, ServerError> {
        self.calls.lock().unwrap().push(service.to_string());
        let name = target.to_string();
        let group = match parameters.get("tableName").and_then(Value::as_str) {
            Some(table) => table.to_string(),
            None => name.rsplit('/').next().unwrap_or_default().to_string(),
        };
        match service {
            "GetConfigurationTable" => Ok(self.tables.get(&group).cloned()),
            "GetGroupMembers" => {
                let mut members = self.members.get(&group).cloned().unwrap_or_default();
                for (entity, added) in self.added.lock().unwrap().iter() {
                    if *entity == group {
                        members.push(added["member"].as_str().unwrap().to_string());
                    }
                }
                Ok(Some(json!({
                    "rows": members.iter().map(|m| {
                        let (kind, name) = m.split_once(':').unwrap_or(("Group", m));
                        json!({"name": name, "type": kind})
                    }).collect::<Vec<_>>()
                })))
            }
            "AddMember" | "AddRunTimePermission" => {
                self.added.lock().unwrap().push((group, parameters.clone()));
                Ok(None)
            }
            _ => Ok(None),
        }
    }
}

impl push::Remote for Fake {
    fn fetch(&self, key: &EntityKey) -> Result<Option<Vec<u8>>, ServerError> {
        let name = key.name();
        Ok(self.exports.get(name).map(|xml| xml.clone().into_bytes()))
    }
    fn import(&self, _: &str, _: &[u8]) -> Result<(), ServerError> {
        self.calls.lock().unwrap().push("import".to_string());
        Ok(())
    }
}

fn temp() -> (tempfile::TempDir, PathBuf) {
    let path_guard = tempfile::Builder::new()
        .prefix("twaco-permissions-server-")
        .tempdir()
        .unwrap();
    let path = path_guard.path().to_path_buf();
    std::fs::create_dir_all(&path).unwrap();
    (path_guard, path)
}

fn write(root: &Path, relative: &str, text: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

const POLICY: &str = r#"
[[role]]
name = "viewer"
group = "Viewer_UG"

[[role]]
name = "admin"
group = "Admin_UG"
includes = ["viewer"]

[[platform]]
member_of = "PTCDTS.Base.Permissions.Default_UG"
roles = ["viewer"]
requires = "PTCDTS.Base.Permissions"

[[platform]]
grant = { entity = "Resources/EntityServices", resource = "ReadEntityDefinitionAsJSON" }
roles = ["admin"]
"#;

/// One Thing visible to both roles, as the repository has it.
const THING: &str = r#"<Entities><Things><Thing name="Acme.App.Manager" projectName="Acme.App"><VisibilityPermissions><Visibility><Principal isPermitted="true" name="Acme.App.Default_OR:Acme.App.Viewer_UG" type="OrganizationalUnit"/><Principal isPermitted="true" name="Acme.App.Default_OR:Acme.App.Admin_UG" type="OrganizationalUnit"/></Visibility></VisibilityPermissions><RunTimePermissions></RunTimePermissions></Thing></Things></Entities>"#;

fn solution() -> (tempfile::TempDir, Solution, PathBuf) {
    let (_dir, root) = temp();
    write(&root, "twaco.toml", "[[project]]\nname = \"Acme.App\"\n");
    write(&root, "permissions.toml", POLICY);
    write(&root, "Things/Acme.App.Manager.xml", THING);
    (
        _dir,
        Solution::load(&root.join("twaco.toml")).unwrap(),
        root,
    )
}

fn visible_to_both() -> Value {
    json!({"Visibility": [
        {"isPermitted": true, "name": "Acme.App.Default_OR:Acme.App.Viewer_UG", "type": "OrganizationalUnit"},
        {"isPermitted": true, "name": "Acme.App.Default_OR:Acme.App.Admin_UG", "type": "OrganizationalUnit"}
    ]})
}

fn organization(units: &[(&str, &str)]) -> String {
    let units: String = units
        .iter()
        .map(|(unit, member)| {
            // `User:name` makes a user member.
            let (kind, member) = member.split_once(':').unwrap_or(("Group", member));
            format!(r#"<OrganizationalUnit name="{unit}"><Members><Members><Member name="{member}" type="{kind}"/></Members></Members></OrganizationalUnit>"#)
        })
        .collect();
    format!(
        r#"<Entities><Organizations><Organization name="Acme.App.Default_OR"><OrganizationalUnits>{units}</OrganizationalUnits></Organization></Organizations></Entities>"#
    )
}

/// The server half's findings: the fixture has no Organization file, so the offline audit warns
/// about the units, which is not what these tests are about.
fn codes(report: &audit::AuditReport) -> Vec<(Severity, &'static str)> {
    report.projects[0]
        .findings
        .iter()
        .filter(|f| f.code != "unknown-principal")
        .map(|f| (f.severity, f.code))
        .collect()
}

#[test]
fn a_server_as_the_policy_says_audits_clean_and_nothing_is_written() {
    let (_dir, solution, _) = solution();
    let mut fake = Fake {
        projects: vec!["PTCDTS.Base.Permissions".to_string()],
        ..Fake::default()
    };
    fake.sets.insert(
        ("Acme.App.Manager".to_string(), "visibility"),
        visible_to_both(),
    );
    fake.sets.insert(
        ("EntityServices".to_string(), "run-time"),
        json!({"permissions": [{"resourceName": "ReadEntityDefinitionAsJSON", "ServiceInvoke": [
            {"isPermitted": true, "name": "Acme.App.Admin_UG", "type": "Group"}]}]}),
    );
    fake.members.insert(
        "PTCDTS.Base.Permissions.Default_UG".to_string(),
        vec![
            "Acme.App.Viewer_UG".to_string(),
            "Acme.App.Admin_UG".to_string(),
        ],
    );
    fake.exports.insert(
        "Acme.App.Default_OR".to_string(),
        organization(&[
            ("Acme.App.Viewer_UG", "Acme.App.Viewer_UG"),
            ("Acme.App.Admin_UG", "Acme.App.Admin_UG"),
        ]),
    );
    let report = audit::audit_with(&solution, None, Some(&fake)).unwrap();
    assert!(report.server);
    assert_eq!(codes(&report), [], "{:#?}", report.projects[0].findings);
    let calls = fake.calls.lock().unwrap();
    assert!(
        !calls.iter().any(|c| c == "set" || c == "import"),
        "{calls:?}"
    );
}

#[test]
fn what_an_import_cannot_carry_is_reported_missing() {
    let (_dir, solution, _) = solution();
    // The project the membership needs is there, but nobody is a member, the grant reaches the
    // viewer only, the server's Manager has an extra grant, and the admin's unit is gone.
    let mut fake = Fake {
        projects: vec!["PTCDTS.Base.Permissions".to_string()],
        ..Fake::default()
    };
    let mut visible = visible_to_both();
    visible["Visibility"]
        .as_array_mut()
        .unwrap()
        .push(json!({"isPermitted": true, "name": "Other_OR", "type": "Organization"}));
    fake.sets
        .insert(("Acme.App.Manager".to_string(), "visibility"), visible);
    fake.sets.insert(
        ("EntityServices".to_string(), "run-time"),
        json!({"permissions": [{"resourceName": "ReadEntityDefinitionAsJSON", "ServiceInvoke": [
            {"isPermitted": true, "name": "Acme.App.Viewer_UG", "type": "Group"}]}]}),
    );
    fake.exports.insert(
        "Acme.App.Default_OR".to_string(),
        organization(&[
            ("Acme.App.Viewer_UG", "Acme.App.Editor_UG"),
            // A user named like the group is not the group.
            ("Acme.App.Admin_UG", "User:Acme.App.Admin_UG"),
        ]),
    );
    let report = audit::audit_with(&solution, None, Some(&fake)).unwrap();
    let found = codes(&report);
    for expected in [
        (Severity::Error, "server-differs"),
        (Severity::Error, "platform-grant-missing"),
        (Severity::Error, "membership-missing"),
        (Severity::Error, "server-unit-without-group"),
    ] {
        assert!(found.contains(&expected), "{expected:?} in {found:?}");
    }
    // Both role groups are missing from the group: the viewer, and the admin who includes it.
    let missing = found
        .iter()
        .filter(|(_, code)| *code == "membership-missing")
        .count();
    assert_eq!(missing, 2);
    let without_group = found
        .iter()
        .filter(|(_, code)| *code == "server-unit-without-group")
        .count();
    assert_eq!(
        without_group, 2,
        "the editor in one unit, a user in the other"
    );

    // Without the project the membership requires, the entry is skipped, and said so.
    fake.projects.clear();
    let report = audit::audit_with(&solution, None, Some(&fake)).unwrap();
    let found = codes(&report);
    assert!(
        found.contains(&(Severity::Note, "platform-skipped")),
        "{found:?}"
    );
    assert!(
        !found.contains(&(Severity::Error, "membership-missing")),
        "{found:?}"
    );
}

#[test]
fn a_missing_organization_or_unit_is_reported_and_read_once() {
    let (_dir, solution, _) = solution();
    let fake = Fake::default();
    let report = audit::audit_with(&solution, None, Some(&fake)).unwrap();
    let missing: Vec<&String> = report.projects[0]
        .findings
        .iter()
        .filter(|f| f.code == "server-unit-missing")
        .map(|f| &f.message)
        .collect();
    assert_eq!(missing.len(), 2, "{missing:#?}");
    assert!(missing[0].contains("no Organization Acme.App.Default_OR"));
}

#[test]
fn a_platform_push_adds_only_what_is_missing_and_reads_it_back() {
    use super::platform::{self, State};
    let (_dir, solution, _) = solution();
    let mut fake = Fake {
        projects: vec!["PTCDTS.Base.Permissions".to_string()],
        ..Fake::default()
    };
    // The viewer is already a member; nothing else is there. A user named like the admin's
    // group is not that group.
    fake.members.insert(
        "PTCDTS.Base.Permissions.Default_UG".to_string(),
        vec![
            "Acme.App.Viewer_UG".to_string(),
            "User:Acme.App.Admin_UG".to_string(),
        ],
    );
    let (loaded, _) = audit::load(&solution, None).unwrap();
    let plan = platform::run(&fake, &loaded, false);
    assert_eq!(plan.count(State::Present), 1);
    assert_eq!(plan.count(State::Missing), 2, "{plan:#?}");
    assert!(
        fake.added.lock().unwrap().is_empty(),
        "a plan writes nothing"
    );

    let applied = platform::run(&fake, &loaded, true);
    assert_eq!(applied.count(State::Added), 2, "{applied:#?}");
    assert_eq!(applied.count(State::Failed), 0);
    let added = fake.added.lock().unwrap().clone();
    assert_eq!(added.len(), 2);
    assert!(added
        .iter()
        .any(|(entity, p)| entity == "PTCDTS.Base.Permissions.Default_UG"
            && p["member"] == "Acme.App.Admin_UG"));
    assert!(added.iter().any(|(entity, p)| entity == "EntityServices"
        && p["resource"] == "ReadEntityDefinitionAsJSON"
        && p["allow"] == true
        && p["principal"] == "Acme.App.Admin_UG"));

    let again = platform::run(&fake, &loaded, true);
    assert_eq!(again.count(State::Present), 3);
    assert_eq!(
        fake.added.lock().unwrap().len(),
        2,
        "nothing is added twice"
    );
}

#[test]
fn a_server_audit_reports_the_entities_audited_and_then_the_entities_read() {
    use crate::core::progress::Recorder;
    let (_dir, solution, _) = solution();
    let fake = Fake::default();
    let recorder = Recorder::default();
    audit::audit_with_progress(&solution, None, Some(&fake), &recorder).unwrap();
    let names: Vec<String> = recorder
        .phases()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(names[0], "auditing entities");
    assert_eq!(names[1], "comparing permissions");
    assert!(recorder.advanced() > 0);
}
