use super::super::baseline::{Baseline, BaselineError};
use super::super::entity_key::{EntityKey, ServiceTarget};
use super::super::normalise;
use super::super::profile::Profile;
use super::super::push;
use super::super::server::{ScriptCheck, ServerError};
use super::*;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::time::Duration;

fn entity(name: &str, script: &str) -> Vec<u8> {
    format!(
            "<Entities><Things><Thing name=\"{name}\" projectName=\"P\"><ThingShape>\
             <ServiceDefinitions><ServiceDefinition name=\"S\"/></ServiceDefinitions>\
             <ServiceImplementations><ServiceImplementation name=\"S\" handlerName=\"Script\">\
             <ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row>\
             <code><![CDATA[{script}]]></code></Row></Rows></ConfigurationTable></ConfigurationTables>\
             </ServiceImplementation></ServiceImplementations></ThingShape></Thing></Things></Entities>"
        )
        .into_bytes()
}

fn project(name: &str, entities: Vec<Entity>, scripts: Vec<Script>) -> ProjectBundle {
    ProjectBundle {
        project: name.to_string(),
        file_name: format!("{name}.xml"),
        bytes: format!("bundle:{name}").into_bytes(),
        entities,
        scripts,
        deploy: None,
        post_import: Vec::new(),
    }
}

fn profile(extra: BTreeMap<String, toml::Value>) -> Profile {
    Profile {
        url: "http://server".into(),
        username: "user".into(),
        password: "password".into(),
        app_key: None,
        extra,
    }
}

fn run_test(
    remote: &dyn Remote,
    baselines: &dyn BaselineStore,
    projects: &[ProjectBundle],
    apply: bool,
    force: bool,
) -> Result<Report, DeployError> {
    run(
        remote,
        baselines,
        &profile(BTreeMap::new()),
        projects,
        apply,
        force,
        false,
    )
}

fn target(name: &str, script: &str) -> Entity {
    Entity {
        collection: "Things".to_string(),
        name: name.to_string(),
        bytes: entity(name, script),
    }
}

#[derive(Default)]
struct MemoryBaseline {
    value: RefCell<Baseline>,
    writes: RefCell<usize>,
    fail_write: bool,
}

impl BaselineStore for MemoryBaseline {
    fn load(&self) -> Result<Baseline, BaselineError> {
        Ok(self.value.borrow().clone())
    }

    fn write(&self, baseline: &Baseline) -> Result<(), BaselineError> {
        *self.writes.borrow_mut() += 1;
        if self.fail_write {
            return Err(BaselineError::Io {
                path: "baseline.json".into(),
                why: "disk full".into(),
            });
        }
        *self.value.borrow_mut() = baseline.clone();
        Ok(())
    }
}

#[derive(Default)]
struct Fake {
    held: Mutex<BTreeMap<(String, String), Vec<u8>>>,
    imports: Mutex<Vec<String>>,
    events: Mutex<Vec<String>>,
    calls: Mutex<Vec<(String, String, Value, Duration)>>,
    import_values: BTreeMap<String, Vec<Entity>>,
    call_values: BTreeMap<String, Vec<Entity>>,
    fail_import: Option<String>,
    fail_service: Option<String>,
    fetch_failures: BTreeSet<(String, String)>,
    mismatch: BTreeSet<(String, String)>,
    parse_unreachable: bool,
    checks: Mutex<usize>,
}

impl push::Remote for Fake {
    fn fetch(&self, key: &EntityKey) -> Result<Option<Vec<u8>>, ServerError> {
        let (collection, name) = (key.collection(), key.name());
        self.events
            .lock()
            .unwrap()
            .push(format!("fetch:{collection}/{name}"));
        let key = (collection.to_string(), name.to_string());
        if self.fetch_failures.contains(&key) {
            return Err(ServerError::Transport {
                method: super::super::server::Method::Get,
                url: format!("http://server/{collection}/{name}"),
                why: "offline".to_string(),
            });
        }
        Ok(self.held.lock().unwrap().get(&key).cloned())
    }

    fn import(&self, file_name: &str, _: &[u8]) -> Result<(), ServerError> {
        self.events
            .lock()
            .unwrap()
            .push(format!("import:{file_name}"));
        self.imports.lock().unwrap().push(file_name.to_string());
        if self.fail_import.as_deref() == Some(file_name) {
            return Err(ServerError::Rejected {
                url: "http://server/Importer".to_string(),
                body: "failed".to_string(),
            });
        }
        for entity in self.import_values.get(file_name).into_iter().flatten() {
            let key = (entity.collection.clone(), entity.name.clone());
            let bytes = if self.mismatch.contains(&key) {
                super::tests::entity(&entity.name, "server_changed();")
            } else {
                entity.bytes.clone()
            };
            self.held.lock().unwrap().insert(key, bytes);
        }
        Ok(())
    }
}

impl Remote for Fake {
    fn check_script(&self, script: &str) -> Result<ScriptCheck, ServerError> {
        *self.checks.lock().unwrap() += 1;
        if self.parse_unreachable {
            return Err(ServerError::Transport {
                method: super::super::server::Method::Post,
                url: "http://server/check".to_string(),
                why: "offline".to_string(),
            });
        }
        Ok(ScriptCheck {
            status: !script.contains("BAD"),
            line_number: 3,
            column_number: 4,
            message: "syntax error".to_string(),
        })
    }

    fn call_service(
        &self,
        target: &ServiceTarget,
        service: &str,
        parameters: &Value,
        timeout: Duration,
    ) -> Result<Option<Value>, ServerError> {
        self.events
            .lock()
            .unwrap()
            .push(format!("call:{target}.{service}"));
        self.calls.lock().unwrap().push((
            target.to_string(),
            service.to_string(),
            parameters.clone(),
            timeout,
        ));
        if self.fail_service.as_deref() == Some(service) {
            return Err(ServerError::Rejected {
                url: format!("http://server/{target}/Services/{service}"),
                body: format!("service said no for {parameters}"),
            });
        }
        for entity in self.call_values.get(service).into_iter().flatten() {
            self.held.lock().unwrap().insert(
                (entity.collection.clone(), entity.name.clone()),
                entity.bytes.clone(),
            );
        }
        Ok(None)
    }
}

fn script(entity: &str, source: &str) -> Script {
    Script {
        entity: entity.to_string(),
        service: "S".to_string(),
        source: source.to_string(),
    }
}

#[test]
fn plan_mode_sends_nothing_and_writes_nothing() {
    let target = target("A", "ok();");
    let projects = [project("A", vec![target], vec![script("A", "ok();")])];
    let remote = Fake::default();
    let baselines = MemoryBaseline::default();
    let report = run_test(&remote, &baselines, &projects, false, false).unwrap();
    assert!(report.imported.is_empty());
    assert!(remote.imports.lock().unwrap().is_empty());
    assert_eq!(*baselines.writes.borrow(), 0);
}

#[test]
fn parse_failure_aborts_before_an_import() {
    let projects = [project(
        "A",
        vec![target("A", "BAD")],
        vec![script("A", "BAD")],
    )];
    let remote = Fake::default();
    let error = run_test(&remote, &MemoryBaseline::default(), &projects, true, false).unwrap_err();
    assert!(matches!(error, DeployError::ParseFailed(_)));
    assert!(remote.imports.lock().unwrap().is_empty());
}

#[test]
fn parse_failures_are_reported_in_input_order() {
    let projects = [project(
        "P",
        vec![],
        vec![
            script("First", "BAD one"),
            script("Good", "ok();"),
            script("Last", "BAD two"),
        ],
    )];
    let error = run_test(
        &Fake::default(),
        &MemoryBaseline::default(),
        &projects,
        false,
        false,
    )
    .unwrap_err();
    let DeployError::ParseFailed(failures) = error else {
        panic!("{error}")
    };
    assert_eq!(
        failures
            .iter()
            .map(|failure| failure.entity.as_str())
            .collect::<Vec<_>>(),
        ["First", "Last"]
    );
}

#[test]
fn an_unreachable_parser_aborts_fail_closed() {
    let projects = [project(
        "A",
        vec![target("A", "ok();")],
        vec![script("A", "ok();")],
    )];
    let remote = Fake {
        parse_unreachable: true,
        ..Fake::default()
    };
    let error = run_test(&remote, &MemoryBaseline::default(), &projects, true, false).unwrap_err();
    assert!(matches!(error, DeployError::ParseUnavailable { .. }));
    assert!(remote.imports.lock().unwrap().is_empty());
}

#[test]
fn a_conflict_aborts_without_force() {
    let working = target("A", "mine();");
    let ancestor = entity("A", "ancestor();");
    let server = entity("A", "theirs();");
    let baselines = MemoryBaseline::default();
    let ancestor = normalise::hash(&ancestor).unwrap();
    baselines
        .value
        .borrow_mut()
        .set("Things", "A", ancestor.clone(), ancestor);
    let remote = Fake::default();
    remote
        .held
        .lock()
        .unwrap()
        .insert(("Things".into(), "A".into()), server);
    let projects = [project("A", vec![working], vec![])];
    let error = run_test(&remote, &baselines, &projects, true, false).unwrap_err();
    assert!(matches!(error, DeployError::Conflicts(_)));
    assert!(remote.imports.lock().unwrap().is_empty());
}

#[test]
fn the_first_fetch_error_in_input_order_is_returned() {
    let projects = [project(
        "P",
        vec![target("First", "a();"), target("Last", "b();")],
        vec![],
    )];
    let remote = Fake {
        fetch_failures: BTreeSet::from([
            ("Things".to_string(), "First".to_string()),
            ("Things".to_string(), "Last".to_string()),
        ]),
        ..Fake::default()
    };
    let error = run_test(&remote, &MemoryBaseline::default(), &projects, false, false).unwrap_err();
    assert!(matches!(
        error,
        DeployError::Server { collection, name, .. }
            if collection == "Things" && name == "First"
    ));
}

#[test]
fn projects_import_in_order_and_first_failure_stops_the_second() {
    let a = target("A", "a();");
    let b = target("B", "b();");
    let projects = [project("A", vec![a], vec![]), project("B", vec![b], vec![])];
    let remote = Fake {
        fail_import: Some("A.xml".to_string()),
        ..Fake::default()
    };
    let error = run_test(&remote, &MemoryBaseline::default(), &projects, true, false).unwrap_err();
    assert!(matches!(error, DeployError::Import { project, .. } if project == "A"));
    assert_eq!(remote.imports.lock().unwrap().as_slice(), ["A.xml"]);
}

#[test]
fn a_later_failure_still_records_the_projects_that_did_import() {
    let a = target("A", "a();");
    let b = target("B", "b();");
    let projects = [
        project("A", vec![a.clone()], vec![]),
        project("B", vec![b], vec![]),
    ];
    let remote = Fake {
        import_values: BTreeMap::from([("A.xml".to_string(), vec![a])]),
        fail_import: Some("B.xml".to_string()),
        ..Fake::default()
    };
    let baselines = MemoryBaseline::default();
    let error = run_test(&remote, &baselines, &projects, true, false).unwrap_err();
    assert!(
        error
            .to_string()
            .ends_with("; already imported and recorded: A"),
        "{error}"
    );
    assert!(matches!(error, DeployError::Import { project, .. } if project == "B"));
    assert_eq!(*baselines.writes.borrow(), 1);
    let baseline = baselines.value.borrow();
    let a = baseline
        .get("Things", "A")
        .expect("A reached the server and was read back");
    assert_eq!(a.local, a.server);
    assert!(baseline.get("Things", "B").is_none(), "B never imported");
}

#[test]
fn a_baseline_that_cannot_be_written_adds_to_the_failure_rather_than_hiding_it() {
    let a = target("A", "a();");
    let b = target("B", "b();");
    let projects = [
        project("A", vec![a.clone()], vec![]),
        project("B", vec![b], vec![]),
    ];
    let remote = Fake {
        import_values: BTreeMap::from([("A.xml".to_string(), vec![a])]),
        fail_import: Some("B.xml".to_string()),
        ..Fake::default()
    };
    let baselines = MemoryBaseline {
        fail_write: true,
        ..MemoryBaseline::default()
    };
    let error = run_test(&remote, &baselines, &projects, true, false).unwrap_err();
    let text = error.to_string();
    assert!(
        text.starts_with("project B import failed"),
        "the import failure leads: {text}"
    );
    assert!(
        text.contains("imported (A) could not be written: "),
        "{text}"
    );
    assert!(text.contains("disk full"), "{text}");
}

#[test]
fn baseline_is_written_once_and_contains_only_matching_read_backs() {
    let same = target("Same", "same();");
    let changed = target("Changed", "changed();");
    let project = project("P", vec![same.clone(), changed.clone()], vec![]);
    let remote = Fake {
        import_values: BTreeMap::from([("P.xml".to_string(), vec![same, changed])]),
        mismatch: BTreeSet::from([("Things".to_string(), "Changed".to_string())]),
        ..Fake::default()
    };
    let baselines = MemoryBaseline::default();
    let error = run_test(&remote, &baselines, &[project], true, false).unwrap_err();
    assert!(matches!(error, DeployError::NotKept(_)));
    assert_eq!(*baselines.writes.borrow(), 1);
    let baseline = baselines.value.borrow();
    assert!(baseline.get("Things", "Same").is_some());
    assert!(baseline.get("Things", "Changed").is_none());
}

#[test]
fn two_projects_import_in_the_given_dependency_order() {
    let a = target("A", "a();");
    let b = target("B", "b();");
    let projects = [
        project("A", vec![a.clone()], vec![]),
        project("B", vec![b.clone()], vec![]),
    ];
    let remote = Fake {
        import_values: BTreeMap::from([
            ("A.xml".to_string(), vec![a]),
            ("B.xml".to_string(), vec![b]),
        ]),
        ..Fake::default()
    };
    run_test(&remote, &MemoryBaseline::default(), &projects, true, false).unwrap();
    assert_eq!(
        remote.imports.lock().unwrap().as_slice(),
        ["A.xml", "B.xml"]
    );
}

#[test]
fn an_applied_deploy_logs_one_info_event_per_phase() {
    let a = target("A", "a();");
    let projects = [project("LogPhaseProject", vec![a.clone()], vec![])];
    let remote = Fake {
        import_values: BTreeMap::from([("LogPhaseProject.xml".to_string(), vec![a])]),
        ..Fake::default()
    };
    let (result, logs) = crate::core::diagnostics::captured(|| {
        let span = tracing::info_span!("capture", mine = "deploy-logs-phases");
        let _entered = span.enter();
        run_test(&remote, &MemoryBaseline::default(), &projects, true, false)
    });
    result.unwrap();
    for phase in [
        "deploy: scripts checked",
        "deploy: planned",
        "deploy: read back",
        "deploy: baseline written",
    ] {
        assert!(
            logs.lines().any(|l| l.contains(phase)
                && l.contains("INFO")
                && l.contains("deploy-logs-phases")),
            "{phase} is missing:\n{logs}"
        );
    }
    assert!(
        logs.lines()
            .any(|l| l.contains("deploy: project imported") && l.contains("LogPhaseProject")),
        "{logs}"
    );
}

fn call(target: &str, service: &str, parameters: Value) -> ServiceCall {
    ServiceCall {
        target: target.into(),
        service: service.into(),
        parameters,
    }
}

#[test]
fn calls_follow_all_import_read_backs_in_project_order_then_entities_are_re_read() {
    let a = target("A", "a();");
    let b = target("B", "b();");
    let mut pa = project("A", vec![a.clone()], vec![]);
    pa.deploy = Some(call("Things/A.Entry", "DeployA", serde_json::json!({})));
    pa.post_import = vec![call("Things/A.Seed", "SeedA", serde_json::json!({}))];
    let mut pb = project("B", vec![b.clone()], vec![]);
    pb.deploy = Some(call("Things/B.Entry", "DeployB", serde_json::json!({})));
    pb.post_import = vec![call("Things/B.Seed", "SeedB", serde_json::json!({}))];
    let remote = Fake {
        import_values: BTreeMap::from([("A.xml".into(), vec![a]), ("B.xml".into(), vec![b])]),
        ..Fake::default()
    };

    run_test(&remote, &MemoryBaseline::default(), &[pa, pb], true, false).unwrap();
    let events = remote.events.lock().unwrap();
    let start = events
        .iter()
        .position(|event| event == "import:A.xml")
        .unwrap();
    let deploy = &events[start..];
    assert_eq!(&deploy[..2], ["import:A.xml", "import:B.xml"]);
    let calls: Vec<&str> = deploy
        .iter()
        .filter(|event| event.starts_with("call:"))
        .map(String::as_str)
        .collect();
    assert_eq!(
        calls,
        [
            "call:Things/A.Entry.DeployA",
            "call:Things/A.Seed.SeedA",
            "call:Things/B.Entry.DeployB",
            "call:Things/B.Seed.SeedB",
        ]
    );
    let first_call = deploy
        .iter()
        .position(|event| event.starts_with("call:"))
        .unwrap();
    let reads_before = deploy[2..first_call]
        .iter()
        .filter(|event| event.starts_with("fetch:"))
        .count();
    let reads_after = deploy[first_call + 4..]
        .iter()
        .filter(|event| event.starts_with("fetch:"))
        .count();
    assert_eq!((reads_before, reads_after), (2, 2));
}

#[test]
fn only_runs_the_deploy_service_and_marks_post_import_skipped() {
    let a = target("A", "a();");
    let mut project = project("A", vec![a.clone()], vec![]);
    project.deploy = Some(call("Things/A", "Deploy", serde_json::json!({})));
    project.post_import = vec![call("Things/A", "Seed", serde_json::json!({}))];
    let remote = Fake {
        import_values: BTreeMap::from([("A.xml".into(), vec![a])]),
        ..Fake::default()
    };
    let report = run(
        &remote,
        &MemoryBaseline::default(),
        &profile(BTreeMap::new()),
        &[project],
        true,
        false,
        true,
    )
    .unwrap();
    assert_eq!(remote.calls.lock().unwrap()[0].1, "Deploy");
    assert_eq!(remote.calls.lock().unwrap()[0].3, Duration::from_secs(300));
    assert_eq!(remote.calls.lock().unwrap().len(), 1);
    assert!(report
        .calls
        .iter()
        .any(|planned| planned.call.service == "Seed" && planned.skipped));
}

#[test]
fn a_failing_deploy_call_stops_later_calls_but_records_import_read_back_once() {
    let a = target("A", "a();");
    let mut project = project("A", vec![a.clone()], vec![]);
    project.deploy = Some(call("Things/A", "Deploy", serde_json::json!({})));
    project.post_import = vec![call("Things/A", "Never", serde_json::json!({}))];
    let remote = Fake {
        import_values: BTreeMap::from([("A.xml".into(), vec![a])]),
        fail_service: Some("Deploy".into()),
        ..Fake::default()
    };
    let baselines = MemoryBaseline::default();
    let error = run_test(&remote, &baselines, &[project], true, false).unwrap_err();
    assert!(error.to_string().contains("Things/A.Deploy"));
    assert!(error.to_string().contains("service said no"));
    assert_eq!(remote.calls.lock().unwrap().len(), 1);
    assert_eq!(*baselines.writes.borrow(), 1);
    assert!(baselines.value.borrow().get("Things", "A").is_some());
}

#[test]
fn placeholders_are_substituted_only_in_the_request_and_unknown_keys_fail_up_front() {
    let secret = "a-value-that-must-not-be-rendered";
    let a = target("A", "a();");
    let mut project = project("SecretProject", vec![a.clone()], vec![]);
    project.deploy = Some(call(
        "Things/A",
        "Deploy",
        serde_json::json!({"deploymentConfig": {"password": "${profile:database_password}"}}),
    ));
    let remote = Fake {
        import_values: BTreeMap::from([("SecretProject.xml".into(), vec![a])]),
        ..Fake::default()
    };
    let active = profile(BTreeMap::from([(
        "database_password".into(),
        toml::Value::String(secret.into()),
    )]));
    let report = run(
        &remote,
        &MemoryBaseline::default(),
        &active,
        &[project.clone()],
        true,
        false,
        false,
    )
    .unwrap();
    assert_eq!(
        remote.calls.lock().unwrap()[0].2["deploymentConfig"]["password"],
        secret
    );
    let rendered = format!("{report:?}");
    assert!(rendered.contains("${profile:database_password}"));
    assert!(!rendered.contains(secret));
    let plan_text = report
        .calls
        .iter()
        .map(|planned| planned.call.to_string())
        .collect::<String>();
    assert!(plan_text.contains("${profile:database_password}"));
    assert!(!plan_text.contains(secret));
    assert!(!format!("{active:?}").contains(secret));

    let rejected = Fake {
        import_values: BTreeMap::from([("SecretProject.xml".into(), vec![target("A", "a();")])]),
        fail_service: Some("Deploy".into()),
        ..Fake::default()
    };
    let error = run(
        &rejected,
        &MemoryBaseline::default(),
        &active,
        &[project.clone()],
        true,
        false,
        false,
    )
    .unwrap_err();
    assert!(error.to_string().contains("${profile:database_password}"));
    assert!(!error.to_string().contains(secret));

    project.deploy.as_mut().unwrap().parameters =
        serde_json::json!({"password": "${profile:missing_key}"});
    let unseen = Fake::default();
    let error = run(
        &unseen,
        &MemoryBaseline::default(),
        &active,
        &[project],
        true,
        false,
        false,
    )
    .unwrap_err();
    let displayed = error.to_string();
    assert!(displayed.contains("missing_key"));
    assert!(displayed.contains("SecretProject"));
    assert!(!displayed.contains(secret));
    assert!(unseen.imports.lock().unwrap().is_empty());
    assert_eq!(*unseen.checks.lock().unwrap(), 0);
    assert!(unseen.events.lock().unwrap().is_empty());
}

#[test]
fn placeholders_inside_a_longer_string_are_substituted_and_redacted() {
    let secret = "s3cr3t-in-a-string";
    let a = target("A", "a();");
    let mut project = project("SecretProject", vec![a.clone()], vec![]);
    project.deploy = Some(call(
        "Things/A",
        "Deploy",
        serde_json::json!({
            "deploymentConfig": "{\"databasePassword\":\"${profile:database_password}\",\"port\":${profile:port}}",
            "url": "jdbc:postgresql://${profile:host}/db?password=${profile:database_password}",
            "port": "${profile:port}",
            "unclosed": "${profile:database_password",
        }),
    ));
    let remote = Fake {
        import_values: BTreeMap::from([("SecretProject.xml".into(), vec![a])]),
        ..Fake::default()
    };
    let active = profile(BTreeMap::from([
        (
            "database_password".into(),
            toml::Value::String(secret.into()),
        ),
        ("host".into(), toml::Value::String("db.local".into())),
        ("port".into(), toml::Value::Integer(5432)),
    ]));
    let report = run(
        &remote,
        &MemoryBaseline::default(),
        &active,
        &[project.clone()],
        true,
        false,
        false,
    )
    .unwrap();
    let sent = remote.calls.lock().unwrap()[0].2.clone();
    assert_eq!(
        sent["deploymentConfig"],
        format!("{{\"databasePassword\":\"{secret}\",\"port\":5432}}")
    );
    assert_eq!(
        sent["url"],
        format!("jdbc:postgresql://db.local/db?password={secret}")
    );
    // A whole-string placeholder keeps the profile value's type.
    assert_eq!(sent["port"], 5432);
    assert_eq!(sent["unclosed"], "${profile:database_password");
    let rendered = format!("{report:?}");
    assert!(rendered.contains("${profile:database_password}"));
    assert!(!rendered.contains(secret));

    let rejected = Fake {
        import_values: BTreeMap::from([("SecretProject.xml".into(), vec![target("A", "a();")])]),
        fail_service: Some("Deploy".into()),
        ..Fake::default()
    };
    let error = run(
        &rejected,
        &MemoryBaseline::default(),
        &active,
        &[project.clone()],
        true,
        false,
        false,
    )
    .unwrap_err();
    assert!(!error.to_string().contains(secret));

    // An array has no one text form: refused before anything is sent, and never shown.
    let listed = profile(BTreeMap::from([(
        "values".into(),
        toml::Value::Array(vec![
            toml::Value::String("secret-a".into()),
            toml::Value::String("secret-b".into()),
        ]),
    )]));
    project.deploy.as_mut().unwrap().parameters = serde_json::json!({"x": "x=${profile:values}"});
    let unseen = Fake::default();
    let error = run(
        &unseen,
        &MemoryBaseline::default(),
        &listed,
        &[project.clone()],
        true,
        false,
        false,
    )
    .unwrap_err();
    assert!(
        matches!(error, DeployError::PlaceholderNotText { .. }),
        "{error}"
    );
    assert!(!error.to_string().contains("secret-a"));
    assert!(unseen.imports.lock().unwrap().is_empty());

    // A date embedded in a string is redacted as it was embedded.
    let dated = profile(BTreeMap::from([(
        "since".into(),
        toml::Value::Datetime("2026-10-07T01:02:03Z".parse().unwrap()),
    )]));
    project.deploy.as_mut().unwrap().parameters =
        serde_json::json!({"x": "since ${profile:since}"});
    let rejected = Fake {
        import_values: BTreeMap::from([("SecretProject.xml".into(), vec![target("A", "a();")])]),
        fail_service: Some("Deploy".into()),
        ..Fake::default()
    };
    let error = run(
        &rejected,
        &MemoryBaseline::default(),
        &dated,
        &[project.clone()],
        true,
        false,
        false,
    )
    .unwrap_err();
    assert!(
        !error.to_string().contains("2026-10-07T01:02:03Z"),
        "{error}"
    );

    project.deploy.as_mut().unwrap().parameters =
        serde_json::json!({"url": "jdbc:x?password=${profile:missing_key}"});
    let unseen = Fake::default();
    let error = run(
        &unseen,
        &MemoryBaseline::default(),
        &active,
        &[project],
        true,
        false,
        false,
    )
    .unwrap_err();
    assert!(error.to_string().contains("missing_key"));
    assert!(unseen.imports.lock().unwrap().is_empty());
}

#[test]
fn re_read_advances_only_changed_server_sides_and_writes_once() {
    let changed = target("Changed", "before();");
    let same = target("Same", "same();");
    let after = target("Changed", "after();");
    let mut project = project("P", vec![changed.clone(), same.clone()], vec![]);
    project.deploy = Some(call("Things/Entry", "Deploy", serde_json::json!({})));
    let remote = Fake {
        import_values: BTreeMap::from([("P.xml".into(), vec![changed.clone(), same.clone()])]),
        call_values: BTreeMap::from([("Deploy".into(), vec![after.clone()])]),
        ..Fake::default()
    };
    let baselines = MemoryBaseline::default();
    let report = run_test(&remote, &baselines, &[project], true, false).unwrap();
    assert_eq!(
        report.changed_by_deploy,
        [("Things".into(), "Changed".into())]
    );
    assert_eq!(*baselines.writes.borrow(), 1);
    let baseline = baselines.value.borrow();
    let changed_entry = baseline.get("Things", "Changed").unwrap();
    assert_eq!(
        changed_entry.local,
        normalise::hash(&changed.bytes).unwrap()
    );
    assert_eq!(changed_entry.server, normalise::hash(&after.bytes).unwrap());
    let same_entry = baseline.get("Things", "Same").unwrap();
    assert_eq!(same_entry.local, same_entry.server);
}
