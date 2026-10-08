use super::super::config::Solution;
use super::super::entity_key::ServiceTarget;
use super::super::profile::Profile;
use super::connection::{read_only_connection, Connection};
use super::execute::{temporary_name, thing_xml};
use super::*;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

fn temp(label: &str) -> (tempfile::TempDir, PathBuf) {
    let root_guard = tempfile::Builder::new()
        .prefix(&format!("twaco-db-{label}-"))
        .tempdir()
        .unwrap();
    let root = root_guard.path().to_path_buf();
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("twaco.toml"), "[[project]]\nname='P'\nroot='.'\n").unwrap();
    (root_guard, root)
}

fn solution(root: &std::path::Path) -> Solution {
    Solution::load(&root.join("twaco.toml")).unwrap()
}

fn profile(password: Option<&str>) -> Profile {
    let mut extra = BTreeMap::new();
    if let Some(password) = password {
        extra.insert(
            "database_password".into(),
            toml::Value::String(password.into()),
        );
    }
    extra.insert(
        "database_user".into(),
        toml::Value::String("profile-user".into()),
    );
    Profile {
        url: "http://server".into(),
        username: "api".into(),
        password: "api-secret".into(),
        app_key: None,
        extra,
    }
}

fn live() -> Value {
    json!({ "configurationTables": { "ConnectionInfo": { "rows": [{
        "jDBCConnectionURL": "jdbc:postgresql://db/name?ssl=true&password=url-secret",
        "jDBCDriverClass": "org.postgresql.Driver", "userName": "thing-user",
        "connectionValidationString": "SELECT NOW()", "maxConnections": 7.0
    }] } } })
}

#[derive(Default)]
struct Fake {
    calls: RefCell<Vec<(String, Value)>>,
    imported: RefCell<Option<String>>,
    fail: RefCell<Option<String>>,
    delete_fails: Cell<bool>,
    exists: Cell<bool>,
}

impl Fake {
    fn fail(&self, service: &str) -> bool {
        self.fail.borrow().as_deref() == Some(service)
    }
}

struct SweepFake {
    things: std::cell::RefCell<Vec<(String, String)>>,
    keep: bool,
}

impl Sweeper for SweepFake {
    fn thing_names(&self) -> Result<Vec<String>, String> {
        Ok(self
            .things
            .borrow()
            .iter()
            .map(|(name, _)| name.clone())
            .collect())
    }
    fn template_of(&self, name: &str) -> Result<String, String> {
        Ok(self
            .things
            .borrow()
            .iter()
            .find(|(thing, _)| thing == name)
            .unwrap()
            .1
            .clone())
    }
    fn delete_thing(&self, name: &str) -> Result<(), String> {
        if !self.keep {
            self.things.borrow_mut().retain(|(thing, _)| thing != name);
        }
        Ok(())
    }
    fn thing_exists(&self, name: &str) -> Result<bool, String> {
        Ok(self.things.borrow().iter().any(|(thing, _)| thing == name))
    }
}

fn sweep_fake(keep: bool) -> SweepFake {
    let things = [
        ("ZZ.Twaco.Sql.0badf00d", "Database"),
        ("ZZ.Twaco.Sql.0badf00e", "Database"),
        // Same prefix, not twaco's generated shape, or not a Database: never touched.
        ("ZZ.Twaco.Sql.mine", "Database"),
        ("ZZ.Twaco.Sql.0badf00f", "GenericThing"),
        ("Acme.Real.Database", "Database"),
    ];
    SweepFake {
        things: std::cell::RefCell::new(
            things
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
        ),
        keep,
    }
}

#[test]
fn a_sweep_plan_lists_only_generated_database_things_and_deletes_nothing() {
    let fake = sweep_fake(false);
    let plan = sweep(&fake, false).unwrap();
    let rows: Vec<(&str, &SweepStatus)> = plan
        .iter()
        .map(|row| (row.name.as_str(), &row.status))
        .collect();
    assert_eq!(
        rows,
        [
            ("ZZ.Twaco.Sql.0badf00d", &SweepStatus::Stale),
            ("ZZ.Twaco.Sql.0badf00e", &SweepStatus::Stale),
            ("ZZ.Twaco.Sql.0badf00f", &SweepStatus::Skipped),
        ]
    );
    assert_eq!(fake.things.borrow().len(), 5, "a plan deletes nothing");
}

#[test]
fn a_sweep_apply_deletes_confirms_and_leaves_everything_else() {
    let fake = sweep_fake(false);
    let applied = sweep(&fake, true).unwrap();
    assert_eq!(
        applied
            .iter()
            .filter(|row| row.status == SweepStatus::Deleted)
            .count(),
        2
    );
    let left: Vec<String> = fake
        .things
        .borrow()
        .iter()
        .map(|(name, _)| name.clone())
        .collect();
    assert_eq!(
        left,
        [
            "ZZ.Twaco.Sql.mine",
            "ZZ.Twaco.Sql.0badf00f",
            "Acme.Real.Database"
        ]
    );
    // A delete that answers success but leaves the Thing is a failure.
    let stubborn = sweep_fake(true);
    let failed = sweep(&stubborn, true).unwrap();
    assert!(failed.iter().any(|row| row.status == SweepStatus::Failed
        && row
            .why
            .as_deref()
            .is_some_and(|why| why.contains("still exists"))));
    assert!(is_temporary_name(&temporary_name()));
    assert!(
        !is_temporary_name("ZZ.Twaco.Sql.0BADF00D") && !is_temporary_name("ZZ.Twaco.Sql.0badf00")
    );
}

impl Remote for Fake {
    fn thing(&self, name: &str) -> Result<Value, String> {
        self.calls
            .borrow_mut()
            .push((format!("GET {name}"), Value::Null));
        Ok(live())
    }

    fn import(&self, file_name: &str, xml: &[u8]) -> Result<(), String> {
        self.calls
            .borrow_mut()
            .push((format!("IMPORT {file_name}"), Value::Null));
        self.exists.set(true);
        self.imported
            .replace(Some(String::from_utf8(xml.to_vec()).unwrap()));
        if self.fail("import") {
            Err("import broke DISTINCT-db-password".into())
        } else {
            Ok(())
        }
    }

    fn call(
        &self,
        target: &ServiceTarget,
        service: &str,
        parameters: &Value,
        _timeout: Duration,
    ) -> Result<Option<Value>, String> {
        self.calls
            .borrow_mut()
            .push((format!("CALL {target} {service}"), parameters.clone()));
        if self.fail(service) {
            return Err(format!("{service} broke DISTINCT-db-password"));
        }
        Ok(match service {
            "EncryptPropertyValue" => Some(json!({ "rows": [{ "result": "encrypted-value" }] })),
            "Run" => Some(json!({ "rows": [{ "one": 1 }] })),
            _ => None,
        })
    }

    fn delete_thing(&self, name: &str) -> Result<(), String> {
        self.calls
            .borrow_mut()
            .push((format!("DELETE {name}"), Value::Null));
        if self.delete_fails.get() {
            Err("delete broke DISTINCT-db-password".into())
        } else {
            self.exists.set(false);
            Ok(())
        }
    }

    fn thing_exists(&self, name: &str) -> Result<bool, String> {
        self.calls
            .borrow_mut()
            .push((format!("EXISTS {name}"), Value::Null));
        Ok(self.exists.get())
    }
}

#[test]
fn xml_escapes_values_splits_cdata_and_never_contains_password() {
    let connection = Connection {
        url: "jdbc:x?x=<&password=not-written".into(),
        driver: "D&\"".into(),
        user: "u<".into(),
        validation: "SELECT 1".into(),
        max_connections: json!(3.0),
    };
    let built = thing_xml("T&\"", "SQLCommand", "a ]]> b", &connection, 5, 6);
    assert!(built.contains("name=\"T&amp;&quot;\""));
    assert!(built.contains("a ]]]]><![CDATA[> b"));
    assert!(built.contains("<password></password>"));
    assert!(
        !built.contains("not-written"),
        "the URL itself is expected here, not a profile password"
    );
}

#[test]
fn a_query_connection_is_read_only_at_the_database_or_refused() {
    let connection = |url: &str, driver: &str| Connection {
        url: url.into(),
        driver: driver.into(),
        user: "u".into(),
        validation: "SELECT 1".into(),
        max_connections: serde_json::json!(2),
    };
    let pg = read_only_connection(connection(
        "jdbc:postgresql://h/db",
        "org.postgresql.Driver",
    ))
    .unwrap();
    assert_eq!(
        pg.url,
        "jdbc:postgresql://h/db?options=-c%20default_transaction_read_only%3Don"
    );
    let with_query = read_only_connection(connection(
        "jdbc:postgresql://h/db?ssl=true",
        "org.postgresql.Driver",
    ))
    .unwrap();
    assert!(with_query
        .url
        .contains("?ssl=true&options=-c%20default_transaction_read_only%3Don"));
    let other = read_only_connection(connection(
        "jdbc:sqlserver://h",
        "com.microsoft.sqlserver.jdbc.SQLServerDriver",
    ))
    .unwrap_err();
    assert!(other.0.contains("cannot enforce read-only"), "{}", other.0);
    let taken = read_only_connection(connection(
        "jdbc:postgresql://h/db?options=-c%20x%3D1",
        "org.postgresql.Driver",
    ))
    .unwrap_err();
    assert!(taken.0.contains("already sets `options`"), "{}", taken.0);
}

#[test]
fn jdbc_password_parameter_is_removed() {
    assert_eq!(
        without_password("jdbc:x?a=1&password=secret&b=2"),
        "jdbc:x?a=1&b=2"
    );
    assert_eq!(without_password("jdbc:x;a=1;PASSWORD=secret"), "jdbc:x;a=1");
    assert_eq!(
        without_password("jdbc:x?notpassword=visible&password=secret"),
        "jdbc:x?notpassword=visible"
    );
}

#[test]
fn plan_reads_only_and_printable_fields_hide_both_passwords() {
    let (_dir, root) = temp("plan");
    let fake = Fake::default();
    let options = Options {
        mode: Mode::Run,
        thing: Some("P.Database".into()),
        apply: false,
        no_transaction: false,
        max_rows: 500,
        timeout: Duration::from_secs(120),
    };
    let report = execute(
        &fake,
        &solution(&root),
        &profile(Some("DISTINCT-db-password")),
        "UPDATE x SET y=1",
        &options,
    )
    .unwrap();
    assert_eq!(
        fake.calls
            .borrow()
            .iter()
            .map(|call| call.0.as_str())
            .collect::<Vec<_>>(),
        ["GET P.Database"]
    );
    assert!(fake.imported.borrow().is_none());
    let output = serde_json::to_string(&report).unwrap();
    assert!(output.contains("UPDATE x SET y=1"));
    assert!(!output.contains("DISTINCT-db-password"));
    assert!(!output.contains("url-secret"));
}

#[test]
fn apply_uses_the_verified_order_and_bodies_then_deletes_and_confirms() {
    let (_dir, root) = temp("apply");
    let fake = Fake::default();
    let options = Options {
        mode: Mode::Run,
        thing: Some("P.Database".into()),
        apply: true,
        no_transaction: true,
        max_rows: 123,
        timeout: Duration::from_secs(9),
    };
    let report = execute(
        &fake,
        &solution(&root),
        &profile(Some("DISTINCT-db-password")),
        "a ]]> b",
        &options,
    )
    .unwrap();
    assert_eq!(report.sql, "COMMIT; a ]]> b");
    let calls = fake.calls.borrow();
    let names: Vec<&str> = calls.iter().map(|call| call.0.as_str()).collect();
    assert_eq!(names[0], "GET P.Database");
    assert!(names[1].starts_with("IMPORT ZZ.Twaco.Sql."));
    assert!(names[2].contains("EncryptionServices EncryptPropertyValue"));
    assert!(names[3].contains("SetConfigurationTable"));
    assert!(names[4].contains("RestartThing"));
    assert!(names[5].ends_with(" Run"));
    // The Thing is looked for first (it may never have been made), then deleted, then confirmed gone.
    assert!(names[6].starts_with("EXISTS ZZ.Twaco.Sql."));
    assert!(names[7].starts_with("DELETE ZZ.Twaco.Sql."));
    assert!(names[8].starts_with("EXISTS ZZ.Twaco.Sql."));
    assert_eq!(calls[2].1, json!({ "data": "DISTINCT-db-password" }));
    let table = &calls[3].1["configurationTable"];
    assert_eq!(table["rows"][0]["password"], "encrypted-value");
    assert_eq!(table["rows"][0]["userName"], "profile-user");
    assert_eq!(
        table["dataShape"]["fieldDefinitions"]["password"]["baseType"],
        "PASSWORD"
    );
    assert_eq!(
        table["dataShape"]["fieldDefinitions"]
            .as_object()
            .unwrap()
            .len(),
        6
    );
    let xml = fake.imported.borrow();
    let xml = xml.as_deref().unwrap();
    assert!(xml.contains("<password></password>"));
    assert!(xml.contains("<![CDATA[COMMIT; a ]]]]><![CDATA[> b]]>"));
    assert!(!xml.contains("DISTINCT-db-password"));
    assert!(!xml.contains("url-secret"));
}

#[test]
fn middle_and_run_failures_still_delete_and_errors_hide_the_password() {
    for service in ["SetConfigurationTable", "Run"] {
        let (_dir, root) = temp(service);
        let fake = Fake::default();
        fake.fail.replace(Some(service.into()));
        let options = Options {
            mode: Mode::Query,
            thing: Some("Db".into()),
            apply: true,
            no_transaction: false,
            max_rows: 10,
            timeout: Duration::from_secs(1),
        };
        let error = execute(
            &fake,
            &solution(&root),
            &profile(Some("DISTINCT-db-password")),
            "SELECT 1",
            &options,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains(service));
        assert!(!error.contains("DISTINCT-db-password"));
        assert!(fake
            .calls
            .borrow()
            .iter()
            .any(|call| call.0.starts_with("DELETE ZZ.Twaco.Sql.")));
        assert!(!fake.exists.get());
    }
}

#[test]
fn failed_delete_is_loud_names_the_thing_and_hides_the_password() {
    let (_dir, root) = temp("delete-fail");
    let fake = Fake::default();
    fake.delete_fails.set(true);
    let options = Options {
        mode: Mode::Run,
        thing: Some("Db".into()),
        apply: true,
        no_transaction: false,
        max_rows: 10,
        timeout: Duration::from_secs(1),
    };
    let error = execute(
        &fake,
        &solution(&root),
        &profile(Some("DISTINCT-db-password")),
        "UPDATE x",
        &options,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("FAILED TO DELETE temporary Thing ZZ.Twaco.Sql."));
    assert!(error.contains("remove it manually"));
    assert!(!error.contains("DISTINCT-db-password"));
}

#[test]
fn a_missing_database_password_refuses_before_any_server_read() {
    let (_dir, root) = temp("password");
    let fake = Fake::default();
    let options = Options {
        mode: Mode::Run,
        thing: Some("Db".into()),
        apply: false,
        no_transaction: false,
        max_rows: 10,
        timeout: Duration::from_secs(1),
    };
    let error = execute(
        &fake,
        &solution(&root),
        &profile(None),
        "UPDATE x",
        &options,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("database_password"));
    assert!(fake.calls.borrow().is_empty());
}

#[test]
fn candidate_resolution_follows_template_inheritance_and_explicit_wins() {
    let (_dir, root) = temp("candidates");
    std::fs::create_dir_all(root.join("ThingTemplates")).unwrap();
    std::fs::create_dir_all(root.join("Things")).unwrap();
    std::fs::write(root.join("ThingTemplates/P.DbBase.xml"), r#"<Entities><ThingTemplates><ThingTemplate name="P.DbBase" projectName="P" baseThingTemplate="Database"/></ThingTemplates></Entities>"#).unwrap();
    std::fs::write(root.join("Things/P.Db.xml"), r#"<Entities><Things><Thing name="P.Db" projectName="P" thingTemplate="P.DbBase"/></Things></Entities>"#).unwrap();
    std::fs::write(root.join("Things/P.Other.xml"), r#"<Entities><Things><Thing name="P.Other" projectName="P" thingTemplate="GenericThing"/></Things></Entities>"#).unwrap();
    let solution = solution(&root);
    assert_eq!(resolve_thing(&solution, None).unwrap(), "P.Db");
    assert_eq!(
        resolve_thing(&solution, Some("Outside.Db")).unwrap(),
        "Outside.Db"
    );
    std::fs::write(root.join("Things/P.Second.xml"), r#"<Entities><Things><Thing name="P.Second" projectName="P" thingTemplate="Acme.Database_TT"/></Things></Entities>"#).unwrap();
    let error = resolve_thing(&solution, None).unwrap_err().to_string();
    assert!(error.contains("P.Db") && error.contains("P.Second"));
    std::fs::remove_file(root.join("Things/P.Db.xml")).unwrap();
    std::fs::remove_file(root.join("Things/P.Second.xml")).unwrap();
    assert!(resolve_thing(&solution, None)
        .unwrap_err()
        .to_string()
        .contains("no database Thing"));
}
