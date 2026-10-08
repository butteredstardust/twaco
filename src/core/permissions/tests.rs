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

fn entity_file(xml: &str) -> (tempfile::TempDir, EntityFile, std::path::PathBuf) {
    let dir_guard = tempfile::Builder::new()
        .prefix("twaco-permissions-")
        .tempdir()
        .unwrap();
    let dir = dir_guard.path().to_path_buf();
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("ZZ.Perm.xml");
    std::fs::write(&path, xml).unwrap();
    (
        dir_guard,
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

/// A ThingTemplate with every set, as an export writes it: instance blocks beside the entity's own.
const TEMPLATE: &str = r#"<ThingTemplate name="T_TT"><RunTimePermissions/><InstanceRunTimePermissions><Permissions resourceName="GetX"><ServiceInvoke><Principal isPermitted="true" name="Viewers" type="Group"/></ServiceInvoke></Permissions></InstanceRunTimePermissions><InstanceDesignTimePermissions><Create/><Read><Principal isPermitted="true" name="Editors" type="Group"/></Read><Update/><Delete/><Metadata/></InstanceDesignTimePermissions><InstanceVisibilityPermissions><Visibility><Principal isPermitted="true" name="O:U" type="OrganizationalUnit"/></Visibility></InstanceVisibilityPermissions></ThingTemplate>"#;

#[test]
fn instance_blocks_are_sets_of_their_own() {
    let sets = from_xml(TEMPLATE.as_bytes()).unwrap();
    assert_eq!(
        sets.keys().map(|k| k.kind().label()).collect::<Vec<_>>(),
        [
            "run-time",
            "instance run-time",
            "instance design-time",
            "instance visibility"
        ]
    );
    assert!(sets[&KindKey::of(Kind::RunTime)].is_empty());
    assert!(sets[&KindKey::of(Kind::InstanceRunTime)][&grant("GetX", "ServiceInvoke", "Viewers")]);
    for key in sets.keys() {
        let kind = key.kind();
        let round = from_json(kind, &to_json(kind, &sets[key])).unwrap();
        assert_eq!(round, sets[key], "{}", kind.label());
    }
    // Instance design time is written with all five actions, as design time is.
    let design = to_json(
        Kind::InstanceDesignTime,
        &sets[&KindKey::of(Kind::InstanceDesignTime)],
    );
    assert_eq!(design.as_object().unwrap().len(), 5);
}

#[test]
fn a_push_writes_a_differing_instance_set_through_its_own_service() {
    let (_dir, entity, _) = entity_file(TEMPLATE);
    let empty_run = json!({"permissions": [{"resourceName": "*", "PropertyRead": [], "PropertyWrite": [],
        "ServiceInvoke": [], "EventInvoke": [], "EventSubscribe": []}]});
    let remote = Fake {
        exists: true,
        sets: Mutex::new(BTreeMap::from([
            ("run-time", empty_run.clone()),
            ("instance run-time", empty_run),
            (
                "instance design-time",
                json!({"Create": [], "Update": [], "Delete": [], "Metadata": [],
                       "Read": [{"isPermitted": true, "name": "Editors", "type": "Group"}]}),
            ),
            (
                "instance visibility",
                json!({"Visibility": [{"isPermitted": true, "name": "O:U", "type": "OrganizationalUnit"}]}),
            ),
        ])),
        ..Fake::default()
    };
    let report = run(&remote, std::slice::from_ref(&entity), true);
    let one = &report.entities[0];
    assert_eq!(one.status, Status::Pushed, "{:?}", one.error);
    assert_eq!(*remote.writes.lock().unwrap(), ["instance run-time"]);
    assert_eq!(one.differences.len(), 1);
    assert_eq!(one.differences[0].change, Change::RepositoryOnly);
}

#[test]
fn diff_names_stale_added_and_flipped_grants_and_writes_nothing() {
    let (_dir, entity, _) = entity_file(ENTITY);
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
}

#[test]
fn push_writes_only_the_differing_sets_and_reads_them_back() {
    let (_dir, entity, _) = entity_file(ENTITY);
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
}

#[test]
fn a_set_that_does_not_read_back_as_written_fails_the_entity() {
    let (_dir, entity, _) = entity_file(ENTITY);
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
}

#[test]
fn an_entity_missing_from_the_server_or_without_permissions_is_said_so() {
    let (_dir, entity, _) = entity_file(ENTITY);
    let absent = Fake::default();
    assert_eq!(
        run(&absent, std::slice::from_ref(&entity), true).entities[0].status,
        Status::NotOnServer
    );
    assert!(absent.writes.lock().unwrap().is_empty());

    let (_dir, entity, _) =
        entity_file(r#"<Things><Thing name="ZZ.Perm"><Owner/></Thing></Things>"#);
    let report = run(&absent, std::slice::from_ref(&entity), true);
    assert_eq!(report.entities[0].status, Status::Unmanaged);
}

#[test]
fn only_the_one_entity_is_read_and_a_document_of_several_is_refused() {
    let two = r#"<Entities><Things><Thing name="A"/><Thing name="B"><RunTimePermissions/></Thing></Things></Entities>"#;
    assert!(from_xml(two.as_bytes()).is_err());
    // A block nested below the entity is not the entity's.
    let nested = r#"<Entities><Things><Thing name="A"><ThingShape><RunTimePermissions/></ThingShape></Thing></Things></Entities>"#;
    assert!(from_xml(nested.as_bytes()).unwrap().is_empty());
}

#[test]
fn a_grant_or_a_block_listed_twice_is_refused() {
    let twice = r#"<Thing name="T"><DesignTimePermissions><Read><Principal isPermitted="true" name="Users" type="Group"/><Principal isPermitted="false" name="Users" type="Group"/></Read></DesignTimePermissions></Thing>"#;
    let message = from_xml(twice.as_bytes()).unwrap_err().to_string();
    assert!(
        message.contains("Read: Group Users is listed twice"),
        "{message}"
    );
    let blocks = r#"<Thing name="T"><VisibilityPermissions/><VisibilityPermissions/></Thing>"#;
    assert!(from_xml(blocks.as_bytes()).is_err());
    let json = json!({"Read": [
        {"isPermitted": true, "name": "Users", "type": "Group"},
        {"isPermitted": false, "name": "Users", "type": "Group"}
    ]});
    assert!(from_json(Kind::DesignTime, &json).is_err());
}

#[test]
fn a_comparison_reports_one_step_per_entity() {
    use crate::core::progress::{Event, Recorder};
    let (_dir, entity, _) = entity_file(ENTITY);
    let remote = Fake {
        exists: true,
        sets: Mutex::new(server_sets()),
        ..Fake::default()
    };
    let recorder = Recorder::default();
    run_with_progress(&remote, std::slice::from_ref(&entity), false, &recorder);
    assert_eq!(
        recorder.phases(),
        [("comparing permissions".to_string(), Some(1))]
    );
    assert_eq!(recorder.advanced(), 1);
    assert!(recorder
        .events()
        .contains(&Event::Message("ZZ.Perm".to_string())));
}
