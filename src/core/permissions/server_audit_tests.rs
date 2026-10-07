//! The server half of the audit, against a fake server.

use super::audit::{self, Severity};
use crate::core::config::Solution;
use crate::core::entity_carry::{self, Kind};
use crate::core::entity_key::ServiceTarget;
use crate::core::server::ServerError;
use crate::core::{config_table, push};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// What the fake server holds: permission sets by entity and set, group members, exports.
#[derive(Default)]
struct Fake {
    sets: BTreeMap<(String, &'static str), Value>,
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
        Ok(collection != "Projects" || self.projects.iter().any(|p| p == name))
    }
    fn get(&self, _: &str, name: &str, kind: Kind) -> Result<Value, ServerError> {
        Ok(self
            .sets
            .get(&(name.to_string(), kind.label()))
            .cloned()
            .unwrap_or_else(|| empty(kind)))
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
        _: &Value,
    ) -> Result<Option<Value>, ServerError> {
        self.calls.lock().unwrap().push(service.to_string());
        let name = target.to_string();
        let group = name.rsplit('/').next().unwrap_or_default().to_string();
        match service {
            "GetGroupMembers" => Ok(Some(json!({
                "rows": self.members.get(&group).cloned().unwrap_or_default()
                    .iter().map(|m| json!({"name": m, "type": "Group"})).collect::<Vec<_>>()
            }))),
            _ => Ok(None),
        }
    }
}

impl push::Remote for Fake {
    fn fetch(&self, _: &str, name: &str) -> Result<Option<Vec<u8>>, ServerError> {
        Ok(self.exports.get(name).map(|xml| xml.clone().into_bytes()))
    }
    fn import(&self, _: &str, _: &[u8]) -> Result<(), ServerError> {
        self.calls.lock().unwrap().push("import".to_string());
        Ok(())
    }
}

fn temp() -> PathBuf {
    let nonce = crate::test_nonce();
    let path = std::env::temp_dir().join(format!(
        "twaco-permissions-server-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
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

fn solution() -> (Solution, PathBuf) {
    let root = temp();
    write(&root, "twaco.toml", "[[project]]\nname = \"Acme.App\"\n");
    write(&root, "permissions.toml", POLICY);
    write(&root, "Things/Acme.App.Manager.xml", THING);
    (Solution::load(&root.join("twaco.toml")).unwrap(), root)
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
        .map(|(unit, member)| format!(r#"<OrganizationalUnit name="{unit}"><Members><Members><Member name="{member}" type="Group"/></Members></Members></OrganizationalUnit>"#))
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
    let (solution, root) = solution();
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
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn what_an_import_cannot_carry_is_reported_missing() {
    let (solution, root) = solution();
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
        organization(&[("Acme.App.Viewer_UG", "Acme.App.Editor_UG")]),
    );
    let report = audit::audit_with(&solution, None, Some(&fake)).unwrap();
    let found = codes(&report);
    for expected in [
        (Severity::Error, "server-differs"),
        (Severity::Error, "platform-grant-missing"),
        (Severity::Error, "membership-missing"),
        (Severity::Error, "server-unit-missing"),
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
    let _ = std::fs::remove_dir_all(root);
}
