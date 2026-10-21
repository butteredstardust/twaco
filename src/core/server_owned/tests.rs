use super::*;
use crate::core::{
    baseline::Baseline,
    config::Solution,
    deploy,
    entity::key::EntityKey,
    profile::Profile,
    progress, push,
    server::{ScriptCheck, ServerError},
    status, workspace,
};
use std::collections::BTreeMap;
use std::sync::Mutex;

fn part(label: &str) -> Vec<Part> {
    vec![Part::try_from(label.to_string()).unwrap()]
}

fn document(kind: &str, body: &str) -> Vec<u8> {
    format!("<Entities universal=\"password\"><{kind}s><{kind} name=\"Acme.Entity\" description=\"ours\" projectName=\"Acme.Project\">{body}</{kind}></{kind}s></Entities>").into_bytes()
}

fn table(name: &str, rows: &str) -> String {
    format!("<ConfigurationTable name=\"{name}\"><DataShape><FieldDefinitions><FieldDefinition name=\"password\" baseType=\"STRING\"/></FieldDefinitions></DataShape><Rows>{rows}</Rows></ConfigurationTable>")
}

#[test]
fn config_accepts_rules_and_refuses_each_invalid_shape_at_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("twaco.toml");
    let valid = "[[project]]\nname='Acme.Project'\n[[server_owned]]\nentities=['Groups/Acme.*_UG']\nparts=['members','run-time-permissions']\n";
    std::fs::write(&path, valid).unwrap();
    let solution = Solution::load(&path).unwrap();
    assert_eq!(
        labels(&parts(&solution.server_owned, "Groups", "Acme.Admin_UG")),
        ["members", "run-time-permissions"]
    );
    assert!(parts(&solution.server_owned, "Things", "Acme.Admin_UG").is_empty());
    for (entities, selected) in [
        ("[]", "['members']"),
        ("['Groups/Acme.Admin_UG']", "[]"),
        ("['Acme.Admin_UG']", "['members']"),
        ("['*/Acme.Entity']", "['members']"),
        ("['Things/']", "['members']"),
        ("['Things/Acme.Entity/child']", "['members']"),
        ("['Things/Acme.?']", "['members']"),
        ("['Things/Acme.[Entity]']", "['members']"),
        ("['Things/Acme.Entity']", "['unknown']"),
        ("['Things/Acme.Entity']", "['property-values:']"),
        ("['Things/Acme.Entity']", "['property-values:a,']"),
        ("['Things/Acme.Entity']", "['configuration-table:']"),
        (
            "['Things/Acme.Entity']",
            "['configuration-table:ConnectionInfo.']",
        ),
        (
            "['Things/Acme.Entity']",
            "['configuration-table:ConnectionInfo.password.extra']",
        ),
    ] {
        std::fs::write(&path, format!("[[project]]\nname='Acme.Project'\n[[server_owned]]\nentities={entities}\nparts={selected}\n")).unwrap();
        let error = Solution::load(&path).unwrap_err().to_string();
        assert!(
            error.contains("kinds:")
                && error.contains("server_owned")
                && error.contains("twaco.toml"),
            "{error}"
        );
    }
    for bad in [
        "entities=['Things/Acme.Entity']\nparts=['members']\nextra=true",
        "entities=['Things/Acme.Entity']",
    ] {
        std::fs::write(
            &path,
            format!("[[project]]\nname='Acme.Project'\n[[server_owned]]\n{bad}"),
        )
        .unwrap();
        assert!(Solution::load(&path).is_err());
    }
}

#[test]
fn every_part_is_omitted_on_both_sides_but_source_changes_still_count() {
    let cases = [
        ("Group", "members", "<Members><Member name=\"Acme.A\"/></Members>".into(), "<Members><Member name=\"Acme.B\"/></Members>".into()),
        ("Organization", "members", "<OrganizationalUnits><OrganizationalUnit name=\"Acme.Unit\"><Members><Member name=\"Acme.A\"/></Members></OrganizationalUnit></OrganizationalUnits>".into(), "<OrganizationalUnits><OrganizationalUnit name=\"Acme.Unit\"><Members/></OrganizationalUnit></OrganizationalUnits>".into()),
        ("Thing", "run-time-permissions", "<RunTimePermissions><Permissions><Read><Principal name=\"Acme.A\" type=\"Group\"/></Read></Permissions></RunTimePermissions>".into(), String::new()),
        ("Thing", "visibility-permissions", "<VisibilityPermissions><VisibilityPermission name=\"Acme.Unit\"/></VisibilityPermissions>".into(), String::new()),
        ("Thing", "property-values", "<ThingProperties><speed><Value>1</Value></speed></ThingProperties>".into(), "<ThingProperties><temperature><Value>2</Value></temperature></ThingProperties>".into()),
        ("Thing", "property-values:speed,temperature", "<ThingProperties><speed><Value>1</Value></speed><temperature><Value>2</Value></temperature></ThingProperties>".into(), "<ThingProperties><speed><Value>3</Value></speed></ThingProperties>".into()),
        ("Thing", "configuration-table:ConnectionInfo.password", format!("<ConfigurationTables>{}</ConfigurationTables>", table("ConnectionInfo", "<Row><password><![CDATA[fixture-a]]></password><host>Acme.Host</host></Row>")), format!("<ConfigurationTables>{}</ConfigurationTables>", table("ConnectionInfo", "<Row><password><![CDATA[fixture-b]]></password><host>Acme.Host</host></Row>"))),
        ("Thing", "configuration-table:Runtime", "<ConfigurationTables/>".into(), format!("<ConfigurationTables>{}</ConfigurationTables>", table("Runtime", "<Row><value>server</value></Row>"))),
    ];
    for (kind, label, left, right) in cases {
        let (left, right) = (document(kind, &left), document(kind, &right));
        let selected = part(label);
        let hash = |bytes: &[u8]| normalise::hash_with_parts(bytes, &selected).unwrap();
        assert_ne!(
            normalise::hash(&left).unwrap(),
            normalise::hash(&right).unwrap(),
            "{label}"
        );
        assert_eq!(hash(&left), hash(&right), "{label}");
        let outside = String::from_utf8(right)
            .unwrap()
            .replace("description=\"ours\"", "description=\"changed\"");
        assert_ne!(hash(&left), hash(outside.as_bytes()), "{label}");
        assert_eq!(
            normalise::hash(&left).unwrap(),
            normalise::hash_with_parts(&left, &[]).unwrap()
        );
    }
}

#[test]
fn column_definition_other_cells_and_nested_lookalikes_remain_compared() {
    let xml = document("Thing", &format!("<ConfigurationTables>{}</ConfigurationTables><ThingShape><RunTimePermissions><Read><Principal name=\"Acme.A\"/></Read></RunTimePermissions></ThingShape>", table("ConnectionInfo", "<Row><password>fixture</password><host>Acme.Host</host></Row>")));
    let selected = [
        part("configuration-table:ConnectionInfo.password"),
        part("run-time-permissions"),
    ]
    .concat();
    let hash = normalise::hash_with_parts(&xml, &selected).unwrap();
    for (from, to) in [
        ("baseType=\"STRING\"", "baseType=\"NUMBER\""),
        ("Acme.Host", "Acme.OtherHost"),
        ("Acme.A", "Acme.B"),
    ] {
        let changed = String::from_utf8(xml.clone()).unwrap().replace(from, to);
        assert_ne!(
            hash,
            normalise::hash_with_parts(changed.as_bytes(), &selected).unwrap()
        );
    }
}

#[test]
fn value_container_attributes_stay_compared_after_its_entries_are_omitted() {
    for (label, body) in [
        ("configuration-table:Runtime", format!("<ConfigurationTables description=\"Acme.A\">{}</ConfigurationTables>", table("Runtime", "<Row><value>server</value></Row>"))),
        ("property-values:speed", "<ThingProperties description=\"Acme.A\"><speed><Value>1</Value></speed></ThingProperties>".into()),
    ] {
        let a = document("Thing", &body);
        let b = document("Thing", &body.replace("Acme.A", "Acme.B"));
        let selected = part(label);
        assert_ne!(normalise::hash_with_parts(&a, &selected).unwrap(), normalise::hash_with_parts(&b, &selected).unwrap());
    }
}

#[test]
fn policy_union_is_deterministic_and_changes_invalidate_only_its_baseline() {
    let rule = |text: &str| {
        toml::from_str::<Solution>(&format!("[[project]]\nname='Acme.Project'\n{text}")).unwrap()
    };
    let a = rule("[[server_owned]]\nentities=['Things/Acme.*']\nparts=['property-values:temperature,speed','members','members']");
    let b = rule("[[server_owned]]\nentities=['Things/Acme.Entity']\nparts=['members','property-values:speed','property-values:temperature']");
    let (a, b) = (
        parts(&a.server_owned, "Things", "Acme.Entity"),
        parts(&b.server_owned, "Things", "Acme.Entity"),
    );
    let xml = document("Thing", "");
    let old = normalise::hash(&xml).unwrap();
    let owned = normalise::hash_with_parts(&xml, &a).unwrap();
    assert_eq!(owned, normalise::hash_with_parts(&xml, &b).unwrap());
    let mut baseline = Baseline::default();
    baseline.set("Things", "Acme.Entity", old.clone(), old.clone());
    assert!(baseline.get_for("Things", "Acme.Entity", &owned).is_none());
    baseline.set("Things", "Acme.Entity", owned.clone(), owned.clone());
    assert!(baseline.get_for("Things", "Acme.Entity", &owned).is_some());
    assert!(baseline.get_for("Things", "Acme.Entity", &old).is_none());
    let changed = normalise::hash_with_parts(&xml, &part("members")).unwrap();
    assert!(baseline
        .get_for("Things", "Acme.Entity", &changed)
        .is_none());
}

#[test]
fn transfers_replace_insert_and_remove_without_touching_other_bytes() {
    let selected = [part("members"), part("run-time-permissions")].concat();
    let ours = document("Group", "<!--keep--><Members><Member name=\"Acme.Local\"/></Members><RunTimePermissions><Read/></RunTimePermissions><Other a='b'/>");
    let theirs = document("Group", "<Members><Member name=\"Acme.Server\"/></Members>");
    let expected = String::from_utf8(ours.clone())
        .unwrap()
        .replace("Acme.Local", "Acme.Server")
        .replace("<RunTimePermissions><Read/></RunTimePermissions>", "");
    assert_eq!(
        preserve(&ours, &theirs, &selected).unwrap(),
        expected.as_bytes()
    );
    let empty = document("Group", "<Other/>");
    let added = preserve(&empty, &theirs, &selected).unwrap();
    assert!(String::from_utf8(added.clone())
        .unwrap()
        .contains("<Other/><Members><Member name=\"Acme.Server\"/></Members>"));
    assert_eq!(preserve(&added, &theirs, &selected).unwrap(), added);
    assert_eq!(preserve(&ours, &theirs, &[]).unwrap(), ours);
    let minimal = br#"<Group name="Acme.Entity"/>"#;
    assert_eq!(
        preserve(minimal, &theirs, &selected).unwrap(),
        br#"<Group name="Acme.Entity"><Members><Member name="Acme.Server"/></Members></Group>"#
    );
}

#[derive(Default)]
struct Fake {
    held: Mutex<Option<Vec<u8>>>,
    exported: Option<Vec<u8>>,
    imports: Mutex<Vec<Vec<u8>>>,
    exports: Mutex<usize>,
}

impl push::Remote for Fake {
    fn fetch(&self, _: &EntityKey) -> Result<Option<Vec<u8>>, ServerError> {
        Ok(self.held.lock().unwrap().clone())
    }
    fn export_owned(&self, _: &EntityKey) -> Result<Option<Vec<u8>>, ServerError> {
        *self.exports.lock().unwrap() += 1;
        Ok(self
            .exported
            .clone()
            .or_else(|| self.held.lock().unwrap().clone()))
    }
    fn import(&self, _: &str, bytes: &[u8]) -> Result<(), ServerError> {
        self.imports.lock().unwrap().push(bytes.to_vec());
        *self.held.lock().unwrap() = Some(bytes.to_vec());
        Ok(())
    }
}

#[test]
fn push_uses_exporter_parts_verifies_readback_and_sends_new_entities_unchanged() {
    let fixtures = [
        ("Group", "members", "<Members><Member name=\"Acme.Local\"/></Members>".into(), "<Members><Member name=\"Acme.Server\"/></Members>".into()),
        ("Thing", "configuration-table:Runtime", format!("<ConfigurationTables>{}</ConfigurationTables>", table("Runtime", "<Row><value>local</value></Row>")), format!("<ConfigurationTables>{}</ConfigurationTables>", table("Runtime", "<Row><value>server</value></Row>"))),
        ("Thing", "configuration-table:ConnectionInfo.password", format!("<ConfigurationTables>{}</ConfigurationTables>", table("ConnectionInfo", "<Row><password>fixture-local</password><host>Acme.Host</host></Row>")), format!("<ConfigurationTables>{}</ConfigurationTables>", table("ConnectionInfo", "<Row><password>fixture-server</password><host>Acme.ServerHost</host></Row>"))),
        ("Thing", "property-values", "<ThingProperties><speed><Value>1</Value></speed></ThingProperties>".into(), "<ThingProperties><speed><Value>2</Value></speed><temperature><Value>3</Value></temperature></ThingProperties>".into()),
    ];
    for (kind, label, ours, theirs) in fixtures {
        let root = tempfile::tempdir().unwrap();
        let local = document(kind, &ours);
        let exported = document(kind, &theirs);
        let get = String::from_utf8(exported.clone())
            .unwrap()
            .replace("Acme.Server", "Acme.GetOnly")
            .replace("description=\"ours\"", "description=\"server\"");
        let fake = Fake {
            held: Mutex::new(Some(get.into_bytes())),
            exported: Some(exported.clone()),
            ..Fake::default()
        };
        let target = push::Target {
            key: EntityKey::new(format!("{kind}s"), "Acme.Entity").unwrap(),
            document: push::EntityDocument {
                file_name: "Acme.Entity.xml",
                bytes: &local,
            },
        };
        let selected = part(label);
        assert!(matches!(
            push::push_with_parts(&fake, root.path(), &target, true, true, &selected).unwrap(),
            push::Outcome::Pushed { created: false }
        ));
        let sent = fake.imports.lock().unwrap()[0].clone();
        assert_eq!(
            sent,
            preserve_for_import(&local, &exported, &selected).unwrap()
        );
        assert_eq!(*fake.exports.lock().unwrap(), 1);
        assert_eq!(
            normalise::hash_with_parts(&local, &selected).unwrap(),
            normalise::hash_with_parts(&sent, &selected).unwrap()
        );
        if label.ends_with(".password") {
            let sent = String::from_utf8(sent).unwrap();
            assert!(
                sent.contains("fixture-server")
                    && sent.contains("Acme.Host")
                    && !sent.contains("Acme.ServerHost")
            );
        }
        let new = Fake::default();
        let root = tempfile::tempdir().unwrap();
        assert!(matches!(
            push::push_with_parts(&new, root.path(), &target, true, false, &selected).unwrap(),
            push::Outcome::Pushed { created: true }
        ));
        assert_eq!(new.imports.lock().unwrap()[0], local);
        assert_eq!(*new.exports.lock().unwrap(), 0);
    }
}

#[test]
fn status_records_matching_unowned_content_and_invalidates_changed_policy() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("twaco.toml");
    std::fs::write(&config, "[[project]]\nname='Acme.Project'\n[[server_owned]]\nentities=['Groups/Acme.Entity']\nparts=['members']").unwrap();
    std::fs::create_dir(root.path().join("Groups")).unwrap();
    let local = document("Group", "<Members/>");
    std::fs::write(root.path().join("Groups/Acme.Entity.xml"), &local).unwrap();
    let solution = Solution::load(&config).unwrap();
    let fake = Fake {
        held: Mutex::new(Some(document(
            "Group",
            "<Members><Member name=\"Acme.Server\"/></Members>",
        ))),
        ..Fake::default()
    };
    let entities = workspace::discover(&solution).entities;
    let mut baseline = Baseline::default();
    let (mut statuses, errors) = status::compute_with_progress(
        &fake,
        &baseline,
        &entities,
        &solution.server_owned,
        &progress::NONE,
    );
    assert!(errors.is_empty());
    assert_eq!(statuses[0].verdict, status::Verdict::NoBaselineSame);
    assert_eq!(statuses[0].server_owned, ["members"]);
    assert_eq!(status::record_matching(&mut baseline, &mut statuses), 1);
    assert_eq!(statuses[0].verdict, status::Verdict::InSync);
    let (statuses, _) =
        status::compute_with_progress(&fake, &baseline, &entities, &[], &progress::NONE);
    assert_eq!(statuses[0].verdict, status::Verdict::NoBaselineDiffers);
    assert!(statuses[0].local_baseline.is_none());
}

impl deploy::Remote for Fake {
    fn check_script(&self, _: &str) -> Result<ScriptCheck, ServerError> {
        unreachable!()
    }
    fn call_service(
        &self,
        _: &crate::core::entity::key::ServiceTarget,
        _: &str,
        _: &serde_json::Value,
        _: std::time::Duration,
    ) -> Result<Option<serde_json::Value>, ServerError> {
        unreachable!()
    }
}

#[derive(Default)]
struct Store(Mutex<Baseline>);
impl deploy::BaselineStore for Store {
    fn load(&self) -> Result<Baseline, crate::core::baseline::BaselineError> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn write(&self, baseline: &Baseline) -> Result<(), crate::core::baseline::BaselineError> {
        *self.0.lock().unwrap() = baseline.clone();
        Ok(())
    }
}

#[test]
fn deploy_plans_allow_owned_drift_refuse_source_drift_and_preserve_bundle_values() {
    let local = document(
        "Thing",
        "<ThingProperties><speed><Value>1</Value></speed></ThingProperties>",
    );
    let remote = document(
        "Thing",
        "<ThingProperties><speed><Value>2</Value></speed></ThingProperties>",
    );
    let fake = Fake {
        held: Mutex::new(Some(remote.clone())),
        ..Fake::default()
    };
    let mut projects = vec![deploy::ProjectBundle {
        project: "Acme.Project".into(),
        file_name: "Acme.Project.xml".into(),
        bytes: local.clone(),
        entities: vec![deploy::Entity {
            collection: "Things".into(),
            name: "Acme.Entity".into(),
            bytes: local.clone(),
            server_owned: part("property-values"),
        }],
        scripts: Vec::new(),
        deploy: None,
        post_import: Vec::new(),
    }];
    let plans = deploy::decide_all(&fake, &Baseline::default(), &projects).unwrap();
    assert_eq!(plans[0].decision, push::Decision::AlreadyThere);
    assert_eq!(plans[0].server_owned, ["property-values"]);
    let changed = String::from_utf8(remote.clone())
        .unwrap()
        .replace("description=\"ours\"", "description=\"changed\"");
    *fake.held.lock().unwrap() = Some(changed.into_bytes());
    assert!(matches!(
        deploy::decide_all(&fake, &Baseline::default(), &projects).unwrap()[0].decision,
        push::Decision::Refuse(_)
    ));
    *fake.held.lock().unwrap() = Some(remote.clone());
    let profile = Profile {
        url: "http://server".into(),
        username: "Acme.User".into(),
        password: "fixture".into(),
        app_key: None,
        extra: BTreeMap::new(),
    };
    let report = deploy::run(
        &fake,
        &Store::default(),
        &profile,
        &projects,
        true,
        false,
        false,
    )
    .unwrap();
    assert_eq!(report.kept.len(), 1);
    assert!(report.changed_by_deploy.is_empty());
    assert_eq!(fake.imports.lock().unwrap()[0], remote);
    assert_eq!(projects[0].bytes, local);
    // A new entity retains its committed initial values and does not ask the Exporter.
    let new = Fake::default();
    let report = deploy::run(
        &new,
        &Store::default(),
        &profile,
        &projects,
        true,
        false,
        false,
    )
    .unwrap();
    assert!(report.plans[0].server_owned.is_empty());
    assert_eq!(new.imports.lock().unwrap()[0], local);
    assert_eq!(*new.exports.lock().unwrap(), 0);
    projects[0].entities[0].server_owned.clear();
    assert!(matches!(
        deploy::decide_all(&fake, &Baseline::default(), &projects).unwrap()[0].decision,
        push::Decision::Refuse(_)
    ));
}

#[test]
fn bundle_splice_preserves_other_entities_and_inserts_only_selected_tables() {
    let local = document("Thing", "<Other/>");
    let peer =
        "<Thing name=\"Acme.Peer\"><ThingProperties><speed>7</speed></ThingProperties></Thing>";
    let bundle = String::from_utf8(local)
        .unwrap()
        .replace("</Things>", &format!("{peer}</Things>"));
    let donor = document(
        "Thing",
        &format!(
            "<ConfigurationTables>{}{}</ConfigurationTables>",
            table("Runtime", "<Row><value>server</value></Row>"),
            table("Source", "<Row><value>unselected</value></Row>")
        ),
    );
    let sent = preserve_in_bundle(
        bundle.as_bytes(),
        &donor,
        &part("configuration-table:Runtime"),
    )
    .unwrap();
    let text = String::from_utf8(sent).unwrap();
    assert!(
        text.contains(peer)
            && text.contains("<Other/>")
            && text.contains("server")
            && !text.contains("unselected")
    );
}

#[test]
fn column_transfer_matches_keyed_rows_and_drops_cells_absent_on_the_server() {
    let ours = document("Thing", &format!("<ConfigurationTables>{}</ConfigurationTables>", table("ConnectionInfo", "<Row><Name>Acme.A</Name><password>local-a</password><host>Acme.LocalA</host></Row><Row><Name>Acme.B</Name><password>local-b</password><host>Acme.LocalB</host></Row>")));
    let donor = document("Thing", &format!("<ConfigurationTables>{}</ConfigurationTables>", table("ConnectionInfo", "<Row><Name>Acme.B</Name><password><![CDATA[server-b]]></password><host>Acme.Other</host></Row><Row><Name>Acme.A</Name><host>Acme.Other</host></Row>")));
    let sent = preserve(
        &ours,
        &donor,
        &part("configuration-table:ConnectionInfo.password"),
    )
    .unwrap();
    let expected = String::from_utf8(ours)
        .unwrap()
        .replace("<password>local-a</password>", "")
        .replace(
            "<password>local-b</password>",
            "<password><![CDATA[server-b]]></password>",
        );
    assert_eq!(sent, expected.as_bytes());
}

#[test]
fn exporter_failure_refuses_before_any_import_and_keeps_the_baseline_absent() {
    struct Unsupported;
    impl push::Remote for Unsupported {
        fn fetch(&self, _: &EntityKey) -> Result<Option<Vec<u8>>, ServerError> {
            Ok(Some(document("Group", "<Other/>")))
        }
        fn import(&self, _: &str, _: &[u8]) -> Result<(), ServerError> {
            panic!("an unsupported Exporter must refuse before import")
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let bytes = document("Group", "<Members/>");
    let target = push::Target {
        key: EntityKey::new("Groups", "Acme.Entity").unwrap(),
        document: push::EntityDocument {
            file_name: "Acme.Entity.xml",
            bytes: &bytes,
        },
    };
    let error = push::push_with_parts(
        &Unsupported,
        dir.path(),
        &target,
        true,
        true,
        &part("members"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("Exporter"));
    assert!(!dir.path().join(".twaco/baseline.json").exists());
}

#[test]
fn encrypted_cells_get_the_import_compatible_envelope_without_changing_source_bytes() {
    let ours = document(
        "Thing",
        &format!(
            "<ConfigurationTables>{}</ConfigurationTables>",
            table("ConnectionInfo", "<Row/>")
        ),
    );
    let ours = String::from_utf8(ours)
        .unwrap()
        .replace(" universal=\"password\"", "");
    let donor = document(
        "Thing",
        &format!(
            "<ConfigurationTables>{}</ConfigurationTables>",
            table(
                "ConnectionInfo",
                "<Row><password>fixture-server</password></Row>"
            )
        ),
    );
    let result = preserve_for_import(
        ours.as_bytes(),
        &donor,
        &part("configuration-table:ConnectionInfo.password"),
    )
    .unwrap();
    assert!(String::from_utf8(result)
        .unwrap()
        .contains("<Entities universal=\"password\">"));
    assert!(ours.starts_with("<Entities>"));
}

#[test]
fn column_preservation_refuses_unmatched_rows_before_sending_anything() {
    let ours = document(
        "Thing",
        &format!(
            "<ConfigurationTables>{}</ConfigurationTables>",
            table(
                "ConnectionInfo",
                "<Row><Name>Acme.Local</Name><password>local</password></Row>"
            )
        ),
    );
    let server = document(
        "Thing",
        &format!(
            "<ConfigurationTables>{}</ConfigurationTables>",
            table(
                "ConnectionInfo",
                "<Row><Name>Acme.Server</Name><password>server</password></Row>"
            )
        ),
    );
    let fake = Fake {
        held: Mutex::new(Some(server)),
        ..Fake::default()
    };
    let dir = tempfile::tempdir().unwrap();
    let target = push::Target {
        key: EntityKey::new("Things", "Acme.Entity").unwrap(),
        document: push::EntityDocument {
            file_name: "Acme.Entity.xml",
            bytes: &ours,
        },
    };
    let error = push::push_with_parts(
        &fake,
        dir.path(),
        &target,
        true,
        true,
        &part("configuration-table:ConnectionInfo.password"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("whole configuration table"));
    assert!(fake.imports.lock().unwrap().is_empty());
}

fn secret_table(rows: &str) -> String {
    format!("<ConfigurationTables><ConfigurationTable name=\"ConnectionInfo\"><DataShape><FieldDefinitions><FieldDefinition name=\"password\" baseType=\"PASSWORD\"/><FieldDefinition name=\"host\" baseType=\"STRING\"/></FieldDefinitions></DataShape><Rows>{rows}</Rows></ConfigurationTable></ConfigurationTables>")
}

#[test]
fn an_encrypted_owned_value_is_never_carried_and_push_refuses_to_clear_it_without_force() {
    let local = document(
        "Thing",
        &secret_table("<Row><password></password><host>Acme.Host</host></Row>"),
    );
    let exported = document(
        "Thing",
        &secret_table("<Row><password>cipher-text</password><host>Acme.Host</host></Row>"),
    );
    let selected = part("configuration-table:ConnectionInfo.password");
    assert_eq!(
        labels(&encrypted(&exported, &selected).unwrap()),
        ["configuration-table:ConnectionInfo.password"]
    );
    let target = push::Target {
        key: EntityKey::new("Things", "Acme.Entity").unwrap(),
        document: push::EntityDocument {
            file_name: "Acme.Entity.xml",
            bytes: &local,
        },
    };
    let fake = Fake {
        held: Mutex::new(Some(exported.clone())),
        exported: Some(exported.clone()),
        ..Fake::default()
    };
    let root = tempfile::tempdir().unwrap();
    // Only the owned cell differs, so the entity is in sync; record that.
    let working = normalise::hash_with_parts(&local, &selected).unwrap();
    let mut baseline = Baseline::default();
    baseline.set("Things", "Acme.Entity", working.clone(), working);
    baseline.write(root.path()).unwrap();
    let changed = String::from_utf8(local.clone())
        .unwrap()
        .replace("Acme.Host", "Acme.NewHost")
        .into_bytes();
    let target = push::Target {
        document: push::EntityDocument {
            file_name: "Acme.Entity.xml",
            bytes: &changed,
        },
        ..target
    };
    let refused =
        push::push_with_parts(&fake, root.path(), &target, true, false, &selected).unwrap();
    assert!(matches!(
        refused,
        push::Outcome::Refused(push::Refusal::EncryptedServerOwned { .. })
    ));
    assert!(fake.imports.lock().unwrap().is_empty(), "nothing was sent");
    push::push_with_parts(&fake, root.path(), &target, true, true, &selected).unwrap();
    let sent = String::from_utf8(fake.imports.lock().unwrap()[0].clone()).unwrap();
    assert!(
        !sent.contains("cipher-text") && sent.contains("Acme.NewHost"),
        "the ciphertext is never sent back"
    );
}

#[test]
fn encrypted_parts_cover_whole_tables_and_named_secret_properties_only() {
    let table = document("Thing", &secret_table(""));
    assert_eq!(
        labels(&encrypted(&table, &part("configuration-table:ConnectionInfo")).unwrap()),
        ["configuration-table:ConnectionInfo"]
    );
    assert!(
        encrypted(&table, &part("configuration-table:ConnectionInfo.host"))
            .unwrap()
            .is_empty()
    );
    let thing = document(
        "Thing",
        "<ThingShape><PropertyDefinitions><PropertyDefinition name=\"secret\" baseType=\"PASSWORD\"/><PropertyDefinition name=\"speed\" baseType=\"NUMBER\"/></PropertyDefinitions></ThingShape>",
    );
    let named = part("property-values:secret,speed");
    let secret = encrypted(&thing, &named).unwrap();
    assert_eq!(labels(&secret), ["property-values:secret"]);
    assert_eq!(labels(&carried(&named, &secret)), ["property-values:speed"]);
    let all = part("property-values");
    assert_eq!(
        labels(&encrypted(&thing, &all).unwrap()),
        ["property-values"]
    );
    assert!(carried(&all, &encrypted(&thing, &all).unwrap()).is_empty());
    let plain = document("Thing", "<ThingShape><PropertyDefinitions><PropertyDefinition name=\"speed\" baseType=\"NUMBER\"/></PropertyDefinitions></ThingShape>");
    assert!(encrypted(&plain, &all).unwrap().is_empty());
}

/// The bundled corpus and the real repositories `TWACO_CORPUS` names, as `tests/corpus.rs` reads
/// them: every entity document in each.
fn corpus_entities() -> Vec<(std::path::PathBuf, Vec<u8>)> {
    let mut roots =
        vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/corpus/acme-orders")];
    if let Some(paths) = std::env::var_os("TWACO_CORPUS") {
        roots.extend(std::env::split_paths(&paths).filter(|p| p.is_dir()));
    }
    let mut files = Vec::new();
    for root in roots {
        let mut pending = vec![root];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                let name = entry.file_name();
                if path.is_dir() {
                    if !name.to_string_lossy().starts_with('.') && name != "target" {
                        pending.push(path);
                    }
                } else if path.extension().is_some_and(|e| e == "xml") {
                    let bytes = std::fs::read(&path).unwrap();
                    // One entity per document, as twaco keeps them; exported bundles are skipped.
                    if crate::core::entity::parse(&bytes).is_ok()
                        && crate::core::normalise::entity_of(&bytes).is_ok()
                    {
                        files.push((path, bytes));
                    }
                }
            }
        }
    }
    files.sort();
    files
}

/// Every part an entity document could have: each kind, and each table directly under its
/// `ConfigurationTables`, read from the parsed entity.
fn every_part(bytes: &[u8]) -> Vec<Part> {
    let mut parts: Vec<Part> = [
        "members",
        "run-time-permissions",
        "visibility-permissions",
        "property-values",
    ]
    .iter()
    .flat_map(|label| part(label))
    .collect();
    let entity = normalise::entity_of(bytes).unwrap();
    for table in children(&entity)
        .filter(|e| e.name == b"ConfigurationTables")
        .flat_map(children)
        .filter(|e| e.name == b"ConfigurationTable")
    {
        let name = attr(table, b"name").map(|n| String::from_utf8_lossy(n).into_owned());
        // A name the selector syntax cannot spell cannot be owned; such a table is left out.
        if let Some(Ok(table)) = name.map(|n| Part::try_from(format!("configuration-table:{n}"))) {
            parts.push(table);
        }
    }
    parts
}

/// The entity's own element with nothing in it: a donor that holds none of any part.
fn empty_donor(bytes: &[u8]) -> Vec<u8> {
    let entity = normalise::entity_of(bytes).unwrap();
    let tag = String::from_utf8_lossy(&entity.name).into_owned();
    let name = attr(&entity, b"name").map_or(String::new(), |n| {
        String::from_utf8_lossy(n)
            .replace('&', "&amp;")
            .replace('"', "&quot;")
            .replace('<', "&lt;")
    });
    format!("<Entities><{tag}s><{tag} name=\"{name}\"></{tag}></{tag}s></Entities>").into_bytes()
}

/// The server's presentation of the same content: no whitespace between tags, outside CDATA.
fn exporter_presentation(bytes: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::new();
    let mut rest = text.as_ref();
    while !rest.is_empty() {
        let (plain, cdata) = match rest.find("<![CDATA[") {
            Some(at) => {
                let end = rest[at..]
                    .find("]]>")
                    .map_or(rest.len(), |end| at + end + 3);
                (&rest[..at], &rest[at..end])
            }
            None => (rest, ""),
        };
        let mut last = 0;
        for (i, _) in plain.match_indices('>') {
            let next = plain[i + 1..]
                .find(|c: char| !c.is_ascii_whitespace())
                .map_or(plain.len(), |n| i + 1 + n);
            // Whitespace up to the next tag, or up to the CDATA section that follows.
            if plain[next..].starts_with('<') || next == plain.len() && !cdata.is_empty() {
                out.push_str(&plain[last..=i]);
                last = next;
            }
        }
        out.push_str(&plain[last..]);
        out.push_str(cdata);
        rest = &rest[plain.len() + cdata.len()..];
    }
    out.into_bytes()
}

type Attributes = Vec<(Vec<u8>, Vec<u8>)>;

/// The entity's attributes and the canonical bytes of each of its children, sorted.
fn children_any_order(bytes: &[u8]) -> (Attributes, Vec<Vec<u8>>) {
    let entity = normalise::comparison_tree(bytes, true).unwrap();
    let mut each: Vec<Vec<u8>> = children(&entity)
        .map(normalise::canonical_element_bytes)
        .collect();
    each.sort();
    (entity.attributes, each)
}

/// A transfer that copied nothing would pass the identity law above. So each owned part is taken
/// out (from a donor that has none of it) and put back (from the original): taking it out changes
/// exactly that part, and putting it back restores the entity. A whole table whose rows the source
/// keys is refused when taking it out would change their structure; only the identity law covers
/// such a table.
#[test]
fn every_owned_part_is_taken_out_and_put_back_alone_over_the_corpus() {
    let mut moved = BTreeMap::new();
    for (path, bytes) in &corpus_entities() {
        let name = path.display();
        let donor = empty_donor(bytes);
        for one in every_part(bytes) {
            let selected = std::slice::from_ref(&one);
            let label = one.label();
            let owned = |b: &[u8]| normalise::hash_with_parts(b, selected).unwrap();
            let Ok(stripped) = preserve(bytes, &donor, selected) else {
                // Owning a table whose rows the source keys is refused when the structure would
                // change; the identity law covers that table.
                continue;
            };
            assert_eq!(
                owned(&stripped),
                owned(bytes),
                "{name}: taking out {label} changed something else"
            );
            if normalise::hash(&stripped).unwrap() == normalise::hash(bytes).unwrap() {
                continue;
            }
            let restored = preserve(&stripped, bytes, selected)
                .unwrap_or_else(|e| panic!("{name}: putting back {label}: {e}"));
            // A part the stripped entity no longer holds at all goes back after its last child:
            // compared by name and content, the entity's own children may come in any order.
            assert_eq!(
                children_any_order(&restored),
                children_any_order(bytes),
                "{name}: putting back {label} did not restore the entity"
            );
            let kind = label.split(':').next().unwrap().to_string();
            *moved.entry(kind).or_insert(0) += 1;
        }
    }
    eprintln!("server-owned corpus: parts taken out and put back by kind: {moved:?}");
    for kind in [
        "members",
        "run-time-permissions",
        "visibility-permissions",
        "property-values",
        "configuration-table",
    ] {
        assert!(
            moved.contains_key(kind),
            "no corpus entity moved {kind}: {moved:?}"
        );
    }
}

#[test]
fn preserving_every_part_from_the_same_content_is_the_identity_over_the_corpus() {
    let mut owned_somewhere = BTreeMap::new();
    let files = corpus_entities();
    for (path, bytes) in &files {
        let parts = every_part(bytes);
        // Do not print entity payloads: a real corpus may hold encrypted values.
        let name = path.display();
        assert_eq!(
            preserve(bytes, bytes, &parts).unwrap_or_else(|e| panic!("{name}: {e}")),
            *bytes,
            "{name}: a transfer from the same bytes changed them"
        );
        let presented = exporter_presentation(bytes);
        let carried = preserve(bytes, &presented, &parts).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(
            crate::core::normalise::normalise(&carried).unwrap(),
            crate::core::normalise::normalise(bytes).unwrap(),
            "{name}: the server's presentation of the same content changed it"
        );
        // Which kinds own something here, so the laws above are not vacuous.
        let whole = crate::core::normalise::comparison_tree(bytes, true).unwrap();
        for one in &parts {
            let mut tree = whole.clone();
            omit(&mut tree, std::slice::from_ref(one));
            if crate::core::normalise::canonical_element_bytes(&tree)
                != crate::core::normalise::canonical_element_bytes(&whole)
            {
                let label = one.label();
                let kind = label.split(':').next().unwrap().to_string();
                *owned_somewhere.entry(kind).or_insert(0) += 1;
            }
        }
    }
    eprintln!(
        "server-owned corpus: {} files; owned content by kind: {owned_somewhere:?}",
        files.len()
    );
    for kind in [
        "members",
        "run-time-permissions",
        "visibility-permissions",
        "property-values",
        "configuration-table",
    ] {
        assert!(
            owned_somewhere.contains_key(kind),
            "no corpus entity owns {kind}: {owned_somewhere:?}"
        );
    }
}
