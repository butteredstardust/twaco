use super::*;
use crate::core::entity::EntityInfo;
use crate::core::server::ServerError;
use std::sync::Mutex;

/// The entity XML of the live probe (2026-10-07), as the repository holds it.
const ENTITY: &str = r#"<Entities><Things><Thing name="ZZ.Perm" projectName="P"><DesignTimePermissions><Create/><Read><Principal isPermitted="true" name="Users" type="Group"/></Read><Update/><Delete/><Metadata/></DesignTimePermissions><RunTimePermissions><Permissions resourceName="*"><PropertyRead><Principal isPermitted="false" name="Users" type="Group"/></PropertyRead><PropertyWrite/><ServiceInvoke><Principal isPermitted="true" name="Users" type="Group"/></ServiceInvoke><EventInvoke/><EventSubscribe/></Permissions><Permissions resourceName="GetPropertyValues"><ServiceInvoke><Principal isPermitted="false" name="Users" type="Group"/></ServiceInvoke></Permissions></RunTimePermissions><VisibilityPermissions><Visibility><Principal isPermitted="true" name="O:U" type="OrganizationalUnit"/></Visibility></VisibilityPermissions></Thing></Things></Entities>"#;

/// What the server held after importing an older version of it: Administrators granted twice
/// more, and Users allowed where the repository now denies.
fn server_sets() -> BTreeMap<&'static str, Value> {
    BTreeMap::from([
        (
            "run-time",
            json!({"permissions": [
                {"resourceName": "GetPropertyValues", "PropertyRead": [], "PropertyWrite": [], "EventInvoke": [], "EventSubscribe": [],
                 "ServiceInvoke": [{"isPermitted": false, "name": "Users", "type": "Group"}]},
                {"resourceName": "*", "PropertyWrite": [], "EventInvoke": [], "EventSubscribe": [],
                 "PropertyRead": [{"isPermitted": true, "name": "Administrators", "type": "Group"},
                                  {"isPermitted": true, "name": "Users", "type": "Group"}],
                 "ServiceInvoke": [{"isPermitted": true, "name": "Users", "type": "Group"}]}
            ]}),
        ),
        (
            "design-time",
            json!({"Create": [], "Update": [], "Delete": [], "Metadata": [],
                   "Read": [{"isPermitted": false, "name": "Administrators", "type": "Group"},
                            {"isPermitted": true, "name": "Users", "type": "Group"}]}),
        ),
        (
            "visibility",
            json!({"Visibility": [{"isPermitted": true, "name": "O:U", "type": "OrganizationalUnit"}]}),
        ),
    ])
}

#[derive(Default)]
struct Fake {
    exists: bool,
    sets: Mutex<BTreeMap<&'static str, Value>>,
    writes: Mutex<Vec<&'static str>>,
    /// A set the server ignores writes to, as a server refusing a principal might.
    ignores: Option<&'static str>,
}

impl Remote for Fake {
    fn exists(&self, _: &str, _: &str) -> Result<bool, ServerError> {
        Ok(self.exists)
    }

    fn get(&self, _: &str, _: &str, kind: Kind) -> Result<Value, ServerError> {
        Ok(self.sets.lock().unwrap()[kind.label()].clone())
    }

    fn set(&self, _: &str, _: &str, kind: Kind, value: &Value) -> Result<(), ServerError> {
        self.writes.lock().unwrap().push(kind.label());
        if self.ignores != Some(kind.label()) {
            self.sets
                .lock()
                .unwrap()
                .insert(kind.label(), value.clone());
        }
        Ok(())
    }

    fn differences(&self, _: &str, _: &str, _: &str) -> Result<usize, ServerError> {
        Ok(0)
    }
}

fn entity_file(xml: &str) -> (EntityFile, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "twaco-permissions-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("ZZ.Perm.xml");
    std::fs::write(&path, xml).unwrap();
    (
        EntityFile {
            path: path.clone(),
            info: EntityInfo {
                collection: "Things".to_string(),
                name: "ZZ.Perm".to_string(),
                project: "P".to_string(),
            },
            found_under: "P".to_string(),
        },
        dir,
    )
}

fn grant(resource: &str, action: &str, principal: &str) -> Grant {
    Grant {
        resource: resource.to_string(),
        action: action.to_string(),
        principal: principal.to_string(),
        principal_type: "Group".to_string(),
    }
}

#[test]
fn the_xml_and_the_json_forms_read_as_the_same_grants() {
    let sets = from_xml(ENTITY.as_bytes()).unwrap();
    assert_eq!(sets.len(), 3);
    let run_time = &sets[&KindKey::of(Kind::RunTime)];
    assert_eq!(run_time.len(), 3);
    assert!(!run_time[&grant("*", "PropertyRead", "Users")]);
    assert!(!run_time[&grant("GetPropertyValues", "ServiceInvoke", "Users")]);
    for key in sets.keys() {
        let kind = key.kind();
        let round = from_json(kind, &to_json(kind, &sets[key])).unwrap();
        assert_eq!(round, sets[key], "{}", kind.label());
    }
}

#[test]
fn a_set_the_xml_does_not_declare_is_not_managed() {
    let xml =
        r#"<Thing name="T"><VisibilityPermissions><Visibility/></VisibilityPermissions></Thing>"#;
    let sets = from_xml(xml.as_bytes()).unwrap();
    assert_eq!(
        sets.keys().map(|k| k.kind().label()).collect::<Vec<_>>(),
        ["visibility"]
    );
    assert!(sets[&KindKey::of(Kind::Visibility)].is_empty());
}

#[test]
fn diff_names_stale_added_and_flipped_grants_and_writes_nothing() {
    let (entity, dir) = entity_file(ENTITY);
    let remote = Fake {
        exists: true,
        sets: Mutex::new(server_sets()),
        ..Fake::default()
    };
    let report = run(&remote, std::slice::from_ref(&entity), false);
    let one = &report.entities[0];
    assert_eq!(one.status, Status::Differs);
    let changes: Vec<_> = one
        .differences
        .iter()
        .map(|d| (d.set.label(), d.change, d.grant.to_string()))
        .collect();
    assert_eq!(
        changes,
        [
            (
                "run-time",
                Change::ServerOnly,
                "* PropertyRead: Group Administrators".to_string()
            ),
            (
                "run-time",
                Change::Flipped,
                "* PropertyRead: Group Users".to_string()
            ),
            (
                "design-time",
                Change::ServerOnly,
                "Read: Group Administrators".to_string()
            ),
        ]
    );
    assert_eq!(
        one.sets.iter().map(|k| k.label()).collect::<Vec<_>>(),
        ["run-time", "design-time"]
    );
    assert!(remote.writes.lock().unwrap().is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn push_writes_only_the_differing_sets_and_reads_them_back() {
    let (entity, dir) = entity_file(ENTITY);
    let remote = Fake {
        exists: true,
        sets: Mutex::new(server_sets()),
        ..Fake::default()
    };
    let report = run(&remote, std::slice::from_ref(&entity), true);
    assert_eq!(
        report.entities[0].status,
        Status::Pushed,
        "{:?}",
        report.entities[0].error
    );
    assert_eq!(*remote.writes.lock().unwrap(), ["run-time", "design-time"]);
    let again = run(&remote, std::slice::from_ref(&entity), false);
    assert_eq!(again.entities[0].status, Status::Same);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_set_that_does_not_read_back_as_written_fails_the_entity() {
    let (entity, dir) = entity_file(ENTITY);
    let remote = Fake {
        exists: true,
        sets: Mutex::new(server_sets()),
        ignores: Some("design-time"),
        ..Fake::default()
    };
    let report = run(&remote, std::slice::from_ref(&entity), true);
    let one = &report.entities[0];
    assert_eq!(one.status, Status::Failed);
    assert!(
        one.error
            .as_deref()
            .unwrap()
            .contains("design-time permissions read back with 1 difference"),
        "{:?}",
        one.error
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn an_entity_missing_from_the_server_or_without_permissions_is_said_so() {
    let (entity, dir) = entity_file(ENTITY);
    let absent = Fake::default();
    assert_eq!(
        run(&absent, std::slice::from_ref(&entity), true).entities[0].status,
        Status::NotOnServer
    );
    assert!(absent.writes.lock().unwrap().is_empty());
    std::fs::remove_dir_all(dir).unwrap();

    let (entity, dir) = entity_file(r#"<Things><Thing name="ZZ.Perm"><Owner/></Thing></Things>"#);
    let report = run(&absent, std::slice::from_ref(&entity), true);
    assert_eq!(report.entities[0].status, Status::Unmanaged);
    std::fs::remove_dir_all(dir).unwrap();
}
