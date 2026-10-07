//! Policies and audits over a small solution written to a temporary folder.

use super::audit::{self, Severity};
use super::policy::{Policy, Principal};
use crate::core::config::Solution;
use std::path::{Path, PathBuf};

const PROJECT: &str = "Acme.App";

fn parse(text: &str) -> Result<Policy, String> {
    Policy::parse(PROJECT, Path::new("permissions.toml"), text).map_err(|e| e.why)
}

const ROLES: &str = r#"
[[role]]
name = "viewer"
group = "Viewer_UG"

[[role]]
name = "editor"
group = "Editor_UG"
includes = ["viewer"]

[[role]]
name = "admin"
group = "Admin_UG"
includes = ["editor"]

[[role]]
name = "everyone"
group = "Default_UG"
org = "organization"
"#;

#[test]
fn a_role_reaches_every_role_that_includes_it() {
    let policy = parse(ROLES).unwrap();
    let names = |role: &str| {
        policy
            .grantees(role)
            .iter()
            .map(|r| r.name.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(names("viewer"), ["viewer", "editor", "admin"]);
    assert_eq!(names("editor"), ["editor", "admin"]);
    assert_eq!(names("admin"), ["admin"]);
    assert_eq!(policy.role("viewer").unwrap().group, "Acme.App.Viewer_UG");
    assert_eq!(
        policy.role("viewer").unwrap().org,
        Some(Principal {
            name: "Acme.App.Default_OR:Acme.App.Viewer_UG".to_string(),
            principal_type: "OrganizationalUnit".to_string()
        })
    );
    assert_eq!(
        policy.role("everyone").unwrap().org,
        Some(Principal {
            name: "Acme.App.Default_OR".to_string(),
            principal_type: "Organization".to_string()
        })
    );
}

#[test]
fn a_policy_that_cannot_mean_one_thing_is_refused() {
    for (text, why) in [
        (
            "[[role]]\nname = \"a\"\ngroup = \"A\"\nincludes = [\"b\"]\n[[role]]\nname = \"b\"\ngroup = \"B\"\nincludes = [\"a\"]\n",
            "includes itself",
        ),
        (
            "[[role]]\nname = \"a\"\ngroup = \"A\"\nincludes = [\"nobody\"]\n",
            "which no [[role]] defines",
        ),
        (
            "[[role]]\nname = \"a\"\ngroup = \"A\"\n[[role]]\nname = \"a\"\ngroup = \"B\"\n",
            "unique",
        ),
        (
            "[[runtime]]\nentities = [\"X\"]\nresources = [\"Get\"]\naction = \"Invoke\"\nroles = []\n",
            "is not one of",
        ),
        (
            "[[runtime]]\nentities = [\"X\"]\nroles = []\n",
            "names no resources",
        ),
        ("project = \"Other\"\n", "names project Other"),
        ("[[role]]\nname = \"a\"\ngroup = \"A\"\ncolour = 1\n", "unknown field"),
        (
            "[[platform]]\nmember_of = \"G\"\ngrant = { entity = \"Resources/X\" }\nroles = []\n",
            "exactly one",
        ),
        (
            "[[platform]]\ngrant = { entity = \"EntityServices\" }\nroles = []\n",
            "Collection/Name",
        ),
    ] {
        let error = parse(text).expect_err(text);
        assert!(error.contains(why), "{text}: {error}");
    }
}

fn temp() -> PathBuf {
    let nonce = crate::test_nonce();
    let path = std::env::temp_dir().join(format!(
        "twaco-permissions-policy-{}-{nonce}",
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

fn principal(name: &str, kind: &str, allowed: bool) -> String {
    format!(r#"<Principal isPermitted="{allowed}" name="{name}" type="{kind}"/>"#)
}

/// A solution with a ThingShape of three services, a mashup, the groups and the organization.
fn solution(shape_grants: &str, mashup_visibility: &str, policy: &str) -> (Solution, PathBuf) {
    let root = temp();
    write(&root, "twaco.toml", "[[project]]\nname = \"Acme.App\"\n");
    // The policy's top-level keys must come before the first table.
    write(&root, "permissions.toml", &format!("{policy}\n{ROLES}"));
    let visible = [
        principal(
            "Acme.App.Default_OR:Acme.App.Viewer_UG",
            "OrganizationalUnit",
            true,
        ),
        principal(
            "Acme.App.Default_OR:Acme.App.Editor_UG",
            "OrganizationalUnit",
            true,
        ),
        principal(
            "Acme.App.Default_OR:Acme.App.Admin_UG",
            "OrganizationalUnit",
            true,
        ),
        principal("Acme.App.Default_OR", "Organization", true),
    ]
    .concat();
    write(
        &root,
        "ThingShapes/Acme.App.Orders_TS.xml",
        &format!(
            r#"<Entities><ThingShapes><ThingShape name="Acme.App.Orders_TS" projectName="Acme.App"><ServiceDefinitions><ServiceDefinition name="GetOrders"/><ServiceDefinition name="DeleteOrder"/><ServiceDefinition name="GetSecret"/></ServiceDefinitions><VisibilityPermissions><Visibility>{visible}</Visibility></VisibilityPermissions><InstanceRunTimePermissions>{shape_grants}</InstanceRunTimePermissions></ThingShape></ThingShapes></Entities>"#
        ),
    );
    write(
        &root,
        "Mashups/Acme.App.Admin_MU.xml",
        &format!(
            r#"<Entities><Mashups><Mashup name="Acme.App.Admin_MU" projectName="Acme.App"><VisibilityPermissions><Visibility>{mashup_visibility}</Visibility></VisibilityPermissions></Mashup></Mashups></Entities>"#
        ),
    );
    for group in ["Viewer_UG", "Editor_UG", "Admin_UG", "Default_UG"] {
        write(
            &root,
            &format!("Groups/Acme.App.{group}.xml"),
            &format!(
                r#"<Entities><Groups><Group name="Acme.App.{group}" projectName="Acme.App"><VisibilityPermissions><Visibility>{visible}</Visibility></VisibilityPermissions></Group></Groups></Entities>"#
            ),
        );
    }
    write(
        &root,
        "Organizations/Acme.App.Default_OR.xml",
        &format!(
            r#"<Entities><Organizations><Organization name="Acme.App.Default_OR" projectName="Acme.App"><VisibilityPermissions><Visibility>{visible}</Visibility></VisibilityPermissions><OrganizationalUnits><OrganizationalUnit name="Acme.App.Viewer_UG"/><OrganizationalUnit name="Acme.App.Editor_UG"/><OrganizationalUnit name="Acme.App.Admin_UG"/></OrganizationalUnits></Organization></Organizations></Entities>"#
        ),
    );
    (Solution::load(&root.join("twaco.toml")).unwrap(), root)
}

const RULES: &str = r#"
strict = ["Orders_TS"]

[[runtime]]
entities = ["Orders_TS"]
resources = ["Get*"]
except = ["GetSecret"]
roles = ["viewer"]

[[runtime]]
entities = ["Orders_TS"]
resources = ["DeleteOrder"]
roles = ["admin"]

[[visibility.rule]]
types = ["Mashup"]
names = ["*Admin*"]
roles = ["admin"]
"#;

fn granted(resource: &str, groups: &[&str]) -> String {
    let principals: String = groups
        .iter()
        .map(|group| principal(&format!("Acme.App.{group}"), "Group", true))
        .collect();
    format!(
        r#"<Permissions resourceName="{resource}"><ServiceInvoke>{principals}</ServiceInvoke></Permissions>"#
    )
}

fn as_policy_says() -> String {
    [
        granted("GetOrders", &["Viewer_UG", "Editor_UG", "Admin_UG"]),
        granted("DeleteOrder", &["Admin_UG"]),
    ]
    .concat()
}

fn admin_only() -> String {
    principal(
        "Acme.App.Default_OR:Acme.App.Admin_UG",
        "OrganizationalUnit",
        true,
    )
}

fn codes(report: &audit::AuditReport) -> Vec<(Severity, &'static str)> {
    report.projects[0]
        .findings
        .iter()
        .map(|f| (f.severity, f.code))
        .collect()
}

#[test]
fn entity_xml_written_as_the_policy_says_audits_clean() {
    let (solution, root) = solution(
        &as_policy_says(),
        &admin_only(),
        &format!("{RULES}\n[[runtime]]\nentities = [\"Orders_TS\"]\nresources = [\"GetSecret\"]\nroles = []\n"),
    );
    let report = audit::audit(&solution, None).unwrap();
    assert_eq!(codes(&report), [], "{:#?}", report.projects[0].findings);
    assert_eq!(report.projects[0].mode, "plain");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn drift_unclassified_services_and_refused_principals_are_errors() {
    // The viewer lost GetOrders, someone granted GetSecret, and the admin mashup is visible to
    // a group, to everyone, and to an org the Solution Framework template left behind.
    let grants = [
        granted("GetOrders", &["Editor_UG", "Admin_UG"]),
        granted("DeleteOrder", &["Admin_UG"]),
        granted("GetSecret", &["Viewer_UG"]),
    ]
    .concat();
    let mashup = [
        admin_only(),
        principal("Acme.App.Viewer_UG", "Group", true),
        principal("Acme.App.Default_OR", "Organization", true),
        principal("PTC.SolutionFramework.Default_OR", "Organization", true),
    ]
    .concat();
    let (solution, root) = solution(
        &grants,
        &mashup,
        &format!("{RULES}\n[visibility]\nremove = [\"PTC.SolutionFramework.*\"]\n"),
    );
    let report = audit::audit(&solution, None).unwrap();
    let found = codes(&report);
    assert!(
        found.contains(&(Severity::Error, "unclassified-service")),
        "{found:?}"
    );
    assert!(found.contains(&(Severity::Error, "visibility-not-an-organization")));
    assert!(found.contains(&(Severity::Error, "removed-principal")));
    let drift: Vec<_> = report.projects[0]
        .findings
        .iter()
        .filter(|f| f.code == "differs-from-policy")
        .collect();
    assert_eq!(drift.len(), 2, "{drift:#?}");
    let shape = drift
        .iter()
        .find(|f| f.entity.as_deref() == Some("ThingShapes/Acme.App.Orders_TS"))
        .unwrap();
    assert_eq!(shape.details.len(), 2, "{:#?}", shape.details);
    assert!(shape.message.contains("instance run-time"));
    // The group is not the policy's to keep or drop, so only the org principals count as drift.
    let mashup = drift
        .iter()
        .find(|f| f.entity.as_deref() == Some("Mashups/Acme.App.Admin_MU"))
        .unwrap();
    assert_eq!(mashup.details.len(), 2, "{:#?}", mashup.details);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn slips_are_warnings_and_denies_are_notes() {
    let grants = [as_policy_says().as_str(), r#"<Permissions resourceName="GetSecret"><ServiceInvoke><Principal isPermitted="false" name="Acme.App.Viewr_UG" type="Group"/></ServiceInvoke></Permissions>"#].concat();
    let (solution, root) = solution(
        &grants,
        &admin_only(),
        &format!("{RULES}\n[[runtime]]\nentities = [\"Ordrs_TS\"]\nresources = [\"GetSecret\"]\nroles = []\n[[runtime]]\nentities = [\"Orders_TS\"]\nresources = [\"Find*\"]\nroles = []\n"),
    );
    let report = audit::audit(&solution, None).unwrap();
    let found = codes(&report);
    for expected in [
        (Severity::Warning, "rule-matches-nothing"),
        (Severity::Warning, "pattern-matches-nothing"),
        (Severity::Warning, "unknown-principal"),
        (Severity::Note, "deny"),
    ] {
        assert!(found.contains(&expected), "{expected:?} in {found:?}");
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn helper_mode_needs_a_helper_and_auto_finds_one() {
    let (solution, root) = solution(&as_policy_says(), &admin_only(), "mode = \"helper\"\n");
    let error = audit::audit(&solution, None).unwrap_err().to_string();
    assert!(
        error.contains("PTCDTS.Base.ComponentPermissionHelper_TT"),
        "{error}"
    );
    write(
        &root,
        "Things/Acme.App.ComponentPermissionHelper.xml",
        r#"<Entities><Things><Thing name="Acme.App.ComponentPermissionHelper" projectName="Acme.App" thingTemplate="PTCDTS.Base.ComponentPermissionHelper_TT"></Thing></Things></Entities>"#,
    );
    let report = audit::audit(&solution, None).unwrap();
    assert_eq!(report.projects[0].mode, "helper");
    assert_eq!(
        report.projects[0].helper.as_deref(),
        Some("Acme.App.ComponentPermissionHelper")
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_solution_without_any_policy_says_where_to_write_one() {
    let (solution, root) = solution("", "", "");
    std::fs::remove_file(root.join("permissions.toml")).unwrap();
    let error = audit::audit(&solution, None).unwrap_err().to_string();
    assert!(error.contains("no permissions.toml"), "{error}");
    assert!(audit::audit(&solution, Some("Nope")).is_err());
    let _ = std::fs::remove_dir_all(root);
}
