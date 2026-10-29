use super::super::baseline::{Baseline, BaselineError};
use super::super::entity::key::{EntityKey, ServiceTarget};
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
        framework: false,
    }
}

fn profile(extra: BTreeMap<String, toml::Value>) -> Profile {
    Profile {
        url: "http://server".into(),
        username: "user".into(),
        password: "password".into(),
        app_key: None,
        sdk: None,
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
        server_owned: Vec::new(),
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
    /// Each import's file name and the document it sent.
    sent: Mutex<Vec<(String, Vec<u8>)>>,
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
    /// A building block's entry point on the server: its componentDeploymentState, `None` while
    /// the server has no such Thing.
    block_state: Mutex<Option<String>>,
    /// What a service answers, by service name.
    replies: BTreeMap<String, Value>,
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

    fn export_owned(&self, key: &EntityKey) -> Result<Option<Vec<u8>>, ServerError> {
        self.fetch(key)
    }

    fn import(&self, file_name: &str, xml: &[u8]) -> Result<(), ServerError> {
        self.events
            .lock()
            .unwrap()
            .push(format!("import:{file_name}"));
        self.imports.lock().unwrap().push(file_name.to_string());
        self.sent
            .lock()
            .unwrap()
            .push((file_name.to_string(), xml.to_vec()));
        if self.fail_import.as_deref() == Some(file_name) {
            return Err(ServerError::Rejected {
                url: "http://server/Importer".to_string(),
                body: "failed".to_string(),
            });
        }
        let mut documents: Vec<((String, String), Vec<u8>)> =
            match self.import_values.get(file_name) {
                Some(entities) => entities
                    .iter()
                    .map(|entity| {
                        let key = (entity.collection.clone(), entity.name.clone());
                        let bytes = if self.mismatch.contains(&key) {
                            super::tests::entity(&entity.name, "server_changed();")
                        } else {
                            entity.bytes.clone()
                        };
                        (key, bytes)
                    })
                    .collect(),
                // A one-entity document, such as an agent deploy imports on its own.
                None => super::super::entity::parse(xml)
                    .map(|info| vec![((info.collection, info.name), xml.to_vec())])
                    .unwrap_or_default(),
            };
        // As the server does: namespaces before agents, each checked against what it holds then,
        // and nothing kept when one is refused.
        documents.sort_by_key(|((collection, _), _)| collection == "AIAgents");
        let mut held = self.held.lock().unwrap();
        let before = held.clone();
        for (key, bytes) in documents {
            held.insert(key, bytes);
            if let Some(why) = ai_refusal(&held) {
                *held = before;
                return Err(ServerError::Http {
                    method: super::super::server::Method::Post,
                    status: 406,
                    url: "http://server/Importer".to_string(),
                    body: why,
                });
            }
        }
        Ok(())
    }
}

/// An agent that allows an item its namespace lacks, which the server refuses either way round.
fn ai_refusal(held: &BTreeMap<(String, String), Vec<u8>>) -> Option<String> {
    use super::super::agents;
    for ((collection, name), bytes) in held {
        if collection != "AIAgents" {
            continue;
        }
        for reference in agents::agent(bytes).ok()?.namespaces {
            let key = ("MCPNamespaces".to_string(), reference.name.clone());
            let Some(namespace) = held.get(&key) else {
                continue;
            };
            let lacks = reference
                .items()
                .without(&agents::namespace(namespace).ok()?.items());
            if !lacks.is_empty() {
                return Some(format!(
                    "{name} allows {lacks:?}, which {} lacks",
                    reference.name
                ));
            }
        }
    }
    None
}

impl Remote for Fake {
    fn list_entity_names(&self, collection: &str) -> Result<Vec<String>, ServerError> {
        Ok(self
            .held
            .lock()
            .unwrap()
            .keys()
            .filter(|(held, _)| held == collection)
            .map(|(_, name)| name.clone())
            .collect())
    }

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
        match service {
            "GetPropertyValues" => {
                return match self.block_state.lock().unwrap().clone() {
                    Some(state) => Ok(Some(
                        serde_json::json!({"rows": [{"componentDeploymentState": state}]}),
                    )),
                    None => Err(ServerError::Http {
                        method: super::super::server::Method::Post,
                        status: 404,
                        url: format!("http://server/{target}"),
                        body: "Entity Not Found".to_string(),
                    }),
                };
            }
            "SetPropertyValues" => {
                let state = parameters
                    .pointer("/values/rows/0/componentDeploymentState")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                *self.block_state.lock().unwrap() = state;
                return Ok(None);
            }
            _ => {}
        }
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
        Ok(self.replies.get(service).cloned())
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
    let DeployError::NotKept(report) = &error else {
        panic!("{error}");
    };
    assert_eq!(
        report.not_kept[0].differences,
        ["Thing[Changed]/ThingShape/ServiceImplementations/ServiceImplementation[S]/ConfigurationTables/ConfigurationTable[Script]/Rows/Row/code: text changed"]
    );
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

#[test]
fn an_applied_deploy_reports_each_phase_with_its_total() {
    use super::super::progress::{Event, Recorder};
    let a = target("A", "a();");
    let b = target("B", "b();");
    let mut pa = project("A", vec![a.clone()], vec![script("A", "a();")]);
    pa.deploy = Some(call("Things/A.Entry", "DeployA", serde_json::json!({})));
    pa.post_import = vec![call("Things/A.Seed", "SeedA", serde_json::json!({}))];
    let pb = project("B", vec![b.clone()], vec![script("B", "b();")]);
    let remote = Fake {
        import_values: BTreeMap::from([("A.xml".into(), vec![a]), ("B.xml".into(), vec![b])]),
        ..Fake::default()
    };
    let recorder = Recorder::default();
    run_with_progress(
        &remote,
        &MemoryBaseline::default(),
        &profile(BTreeMap::new()),
        &[pa, pb],
        RunOptions {
            apply: true,
            force: false,
            only: false,
            redeploy_blocks: false,
        },
        &recorder,
    )
    .unwrap();
    let phases: Vec<(String, Option<u64>)> = [
        ("checking scripts", 2),
        ("comparing entities", 2),
        ("importing", 2),
        ("reading back", 2),
        ("calling services", 2),
        ("reading back again", 2),
    ]
    .map(|(name, total)| (name.to_string(), Some(total)))
    .to_vec();
    assert_eq!(recorder.phases(), phases);
    // Every phase advances by exactly its total, and ends.
    assert_eq!(recorder.advanced(), 12);
    let events = recorder.events();
    assert_eq!(
        events.iter().filter(|e| **e == Event::Finish).count(),
        phases.len()
    );
    // Messages hold names only.
    for event in &events {
        if let Event::Message(text) = event {
            assert!(
                ["A", "B"].contains(&text.as_str()),
                "unexpected message {text:?}"
            );
        }
    }
}

#[test]
fn a_plan_reports_no_import_phase() {
    use super::super::progress::Recorder;
    let a = target("A", "a();");
    let remote = Fake::default();
    let recorder = Recorder::default();
    run_with_progress(
        &remote,
        &MemoryBaseline::default(),
        &profile(BTreeMap::new()),
        &[project("A", vec![a], vec![])],
        RunOptions {
            apply: false,
            force: false,
            only: false,
            redeploy_blocks: false,
        },
        &recorder,
    )
    .unwrap();
    let names: Vec<String> = recorder.phases().into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, ["checking scripts", "comparing entities"]);
}

#[test]
fn a_profile_patch_reaches_the_bundle_and_its_entity_but_not_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("Mashups")).unwrap();
    let config = |entity: &str| {
        format!(
            "[[project]]\nname = \"P\"\n[profiles.qa]\n[[profiles.qa.patches]]\n\
             entity = \"{entity}\"\npath = \"UI.Properties.Master\"\nvalue = \"P.Qa\"\n"
        )
    };
    std::fs::write(root.join("twaco.toml"), config("Mashups/P.Main")).unwrap();
    let working = "<Entities><Mashups><Mashup name=\"P.Main\" projectName=\"P\"><mashupContent><![CDATA[{\"UI\":{\"Properties\":{\"Master\":\"P.Dev\"}}}]]></mashupContent></Mashup></Mashups></Entities>";
    let file = root.join("Mashups/P.Main.xml");
    std::fs::write(&file, working).unwrap();
    let options = || PlanOptions {
        only_projects: &[],
        only: &[],
        backend_only: false,
        changed_since: None,
        exclude: &[],
    };
    let solution = super::super::config::Solution::load(&root.join("twaco.toml")).unwrap();

    let (qa, _) = plan_bundles(&solution, "qa", options()).unwrap();
    let bundle = String::from_utf8(qa[0].bytes.clone()).unwrap();
    let entity = String::from_utf8(qa[0].entities[0].bytes.clone()).unwrap();
    assert!(
        bundle.contains("P.Qa") && !bundle.contains("P.Dev"),
        "{bundle}"
    );
    assert!(entity.contains("P.Qa"), "{entity}");
    let (default, _) = plan_bundles(&solution, "default", options()).unwrap();
    assert!(String::from_utf8_lossy(&default[0].bytes).contains("P.Dev"));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), working);

    std::fs::write(root.join("twaco.toml"), config("Mashups/P.Typo")).unwrap();
    let solution = super::super::config::Solution::load(&root.join("twaco.toml")).unwrap();
    let error = plan_bundles(&solution, "qa", options()).unwrap_err();
    assert!(error.contains("Mashups/P.Typo"), "{error}");
}

#[test]
fn only_and_changed_since_are_two_ways_of_choosing_and_are_refused_together() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
    let solution = super::super::config::Solution::load(&dir.path().join("twaco.toml")).unwrap();
    let error = plan_bundles(
        &solution,
        "default",
        PlanOptions {
            only_projects: &[],
            only: &["Things/P.T".to_string()],
            backend_only: false,
            changed_since: Some("HEAD"),
            exclude: &[],
        },
    )
    .unwrap_err();
    assert!(
        error.contains("--only") && error.contains("--changed-since"),
        "{error}"
    );
}

fn namespace_entity(tools: &[&str], resources: &[&str]) -> Entity {
    let tools: String = tools
        .iter()
        .map(|tool| {
            format!(
                "<MCPTool name=\"{tool}\" description=\"d\" serviceName=\"S\" serviceProvider=\"T\" serviceProviderType=\"Thing\"/>"
            )
        })
        .collect();
    let resources: String = resources
        .iter()
        .map(|resource| {
            format!("<MCPResource name=\"{resource}\" description=\"d\" type=\"Service\"/>")
        })
        .collect();
    Entity {
        server_owned: Vec::new(),
        collection: "MCPNamespaces".to_string(),
        name: "N".to_string(),
        bytes: format!(
            "<Entities><MCPNamespaces><MCPNamespace name=\"N\" projectName=\"P\"><MCPTools>{tools}</MCPTools><MCPResources>{resources}</MCPResources></MCPNamespace></MCPNamespaces></Entities>"
        )
        .into_bytes(),
    }
}

fn agent_named(name: &str, tools: &[&str], resources: &[&str]) -> Entity {
    let tools: String = tools
        .iter()
        .map(|tool| format!("<Tool name=\"{tool}\"></Tool>"))
        .collect();
    let resources: String = resources
        .iter()
        .map(|resource| format!("<Resource name=\"{resource}\"></Resource>"))
        .collect();
    Entity {
        server_owned: Vec::new(),
        collection: "AIAgents".to_string(),
        name: name.to_string(),
        bytes: format!(
            "<Entities><AIAgents><AIAgent name=\"{name}\" projectName=\"P\"><MCPNamespaceReferences><MCPNamespaceReference name=\"N\"><AllowedTools>{tools}</AllowedTools><AllowedResources>{resources}</AllowedResources></MCPNamespaceReference></MCPNamespaceReferences></AIAgent></AIAgents></Entities>"
        )
        .into_bytes(),
    }
}

fn agent_entity(tools: &[&str]) -> Entity {
    agent_named("A", tools, &[])
}

type Held = BTreeMap<(String, String), Vec<u8>>;

fn holding(entities: &[Entity]) -> Mutex<Held> {
    Mutex::new(
        entities
            .iter()
            .map(|entity| {
                (
                    (entity.collection.clone(), entity.name.clone()),
                    entity.bytes.clone(),
                )
            })
            .collect(),
    )
}

fn tools_of(remote: &Fake, collection: &str, name: &str) -> Vec<String> {
    let held = remote.held.lock().unwrap();
    let bytes = &held[&(collection.to_string(), name.to_string())];
    if collection == "AIAgents" {
        super::super::agents::agent(bytes).unwrap().namespaces[0]
            .tools
            .clone()
    } else {
        super::super::agents::namespace(bytes)
            .unwrap()
            .tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect()
    }
}

/// The server holds namespace N with `keep` and `gone` and agent A allowing both; the
/// repository drops `gone` from both, in one project or in `PN` and `PA`.
fn dropping_an_allowed_tool(projects: &[&str]) -> (Fake, Vec<ProjectBundle>) {
    let (namespace, agent) = (namespace_entity(&["keep"], &[]), agent_entity(&["keep"]));
    let bundles = match projects {
        [one] => vec![project(one, vec![namespace.clone(), agent.clone()], vec![])],
        _ => projects
            .iter()
            .map(|name| {
                let entity = if *name == "PN" { &namespace } else { &agent };
                project(name, vec![entity.clone()], vec![])
            })
            .collect(),
    };
    let import_values = bundles
        .iter()
        .map(|bundle| (bundle.file_name.clone(), bundle.entities.clone()))
        .collect();
    let remote = Fake {
        held: holding(&[
            namespace_entity(&["keep", "gone"], &[]),
            agent_entity(&["keep", "gone"]),
        ]),
        import_values,
        ..Fake::default()
    };
    (remote, bundles)
}

#[test]
fn the_fake_server_refuses_dropping_an_allowed_tool_in_bundle_order() {
    let (remote, _) = dropping_an_allowed_tool(&["P"]);
    let error = push::Remote::import(&remote, "P.xml", b"bundle").unwrap_err();
    assert!(error.to_string().contains("A allows"), "{error}");
    assert_eq!(tools_of(&remote, "MCPNamespaces", "N"), ["keep", "gone"]);
}

#[test]
fn an_agent_stops_allowing_what_its_namespace_drops_before_the_bundle_imports() {
    let (remote, projects) = dropping_an_allowed_tool(&["P"]);
    let planned = run_test(&remote, &MemoryBaseline::default(), &projects, false, true).unwrap();
    assert_eq!(
        planned.agents_narrowed,
        [AgentNarrowing {
            name: "A".to_string(),
            project: "P".to_string(),
            dropped: vec![DroppedItems {
                namespace: "N".to_string(),
                tools: vec!["gone".to_string()],
                resources: Vec::new(),
                prompts: Vec::new(),
            }],
        }]
    );
    assert!(
        remote.imports.lock().unwrap().is_empty(),
        "a plan imports nothing"
    );

    run_test(&remote, &MemoryBaseline::default(), &projects, true, true).unwrap();
    assert_eq!(
        remote.imports.lock().unwrap().as_slice(),
        ["A.xml", "P.xml"]
    );
    assert_eq!(tools_of(&remote, "MCPNamespaces", "N"), ["keep"]);
    assert_eq!(tools_of(&remote, "AIAgents", "A"), ["keep"]);
}

#[test]
fn either_project_order_deploys() {
    for order in [["PN", "PA"], ["PA", "PN"]] {
        let (remote, projects) = dropping_an_allowed_tool(&order);
        run_test(&remote, &MemoryBaseline::default(), &projects, true, true).unwrap();
        assert_eq!(
            tools_of(&remote, "MCPNamespaces", "N"),
            ["keep"],
            "{order:?}"
        );
        assert_eq!(tools_of(&remote, "AIAgents", "A"), ["keep"], "{order:?}");
    }
}

#[test]
fn a_failed_bundle_puts_the_narrowed_agent_back() {
    let (mut remote, projects) = dropping_an_allowed_tool(&["P"]);
    remote.fail_import = Some("P.xml".to_string());
    let error = run_test(&remote, &MemoryBaseline::default(), &projects, true, true).unwrap_err();
    assert!(
        error
            .to_string()
            .ends_with("; AIAgents/A was put back as the server had it"),
        "{error}"
    );
    assert_eq!(
        remote.imports.lock().unwrap().as_slice(),
        ["A.xml", "P.xml", "A.xml"]
    );
    assert_eq!(tools_of(&remote, "AIAgents", "A"), ["keep", "gone"]);
}

#[test]
fn an_agent_stays_narrowed_once_its_namespace_imported() {
    let (mut remote, projects) = dropping_an_allowed_tool(&["PN", "PA"]);
    remote.fail_import = Some("PA.xml".to_string());
    let baselines = MemoryBaseline::default();
    let old = normalise::hash_with_parts(&agent_entity(&["keep", "gone"]).bytes, &[]).unwrap();
    baselines
        .value
        .borrow_mut()
        .set("AIAgents", "A", old.clone(), old.clone());
    let error = run_test(&remote, &baselines, &projects, true, true).unwrap_err();
    assert!(
        error.to_string().ends_with(
            "; AIAgents/A stays without tool gone from N, which its namespace no longer has; the baseline records that"
        ),
        "{error}"
    );
    // No put-back, which the server would refuse.
    assert_eq!(
        remote.imports.lock().unwrap().as_slice(),
        ["A.xml", "PN.xml", "PA.xml"]
    );
    let narrowed = remote.held.lock().unwrap()[&("AIAgents".to_string(), "A".to_string())].clone();
    let entry = baselines
        .value
        .borrow()
        .get("AIAgents", "A")
        .cloned()
        .unwrap();
    assert_eq!(entry.local, old);
    assert_eq!(
        entry.server,
        normalise::hash_with_parts(&narrowed, &[]).unwrap()
    );
}

#[test]
fn a_failed_narrowing_imports_no_project() {
    let (mut remote, projects) = dropping_an_allowed_tool(&["P"]);
    remote.fail_import = Some("A.xml".to_string());
    let error = run_test(&remote, &MemoryBaseline::default(), &projects, true, true).unwrap_err();
    assert!(
        matches!(&error, DeployError::Narrowing { agent, put_back, .. } if agent == "A" && put_back.is_empty()),
        "{error}"
    );
    assert_eq!(remote.imports.lock().unwrap().as_slice(), ["A.xml"]);
}

#[test]
fn an_agent_outside_the_deploy_that_allows_a_dropped_item_refuses_it() {
    let (remote, projects) = dropping_an_allowed_tool(&["P"]);
    remote.held.lock().unwrap().insert(
        ("AIAgents".to_string(), "B".to_string()),
        agent_named("B", &["gone"], &[]).bytes,
    );
    let error = run_test(&remote, &MemoryBaseline::default(), &projects, false, true).unwrap_err();
    assert!(
        matches!(&error, DeployError::AgentsOutside(agents) if agents.len() == 1 && agents[0].name == "B"),
        "{error}"
    );
    assert!(
        error
            .to_string()
            .contains("AIAgents/B allows tool gone from N"),
        "{error}"
    );
    assert!(remote.imports.lock().unwrap().is_empty());
}

#[test]
fn a_tool_and_a_resource_of_one_name_are_told_apart() {
    let namespace = namespace_entity(&["keep"], &[]);
    let agent = agent_named("A", &["keep"], &[]);
    let remote = Fake {
        held: holding(&[
            namespace_entity(&["keep", "x"], &["x"]),
            agent_named("A", &["keep", "x"], &["x"]),
        ]),
        import_values: BTreeMap::from([(
            "P.xml".to_string(),
            vec![namespace.clone(), agent.clone()],
        )]),
        ..Fake::default()
    };
    let projects = [project("P", vec![namespace, agent], vec![])];
    let report = run_test(&remote, &MemoryBaseline::default(), &projects, true, true).unwrap();
    let dropped = &report.agents_narrowed[0].dropped[0];
    assert_eq!(dropped.tools, ["x"]);
    assert_eq!(dropped.resources, ["x"]);
    assert_eq!(dropped.to_string(), "tool x; resource x from N");
}

#[test]
fn an_exporter_copy_with_self_closed_tags_is_narrowed() {
    let exported = b"<Entities><AIAgents><AIAgent name=\"A\"><ConfigurationTables><ConfigurationTable name=\"TaxonomyConfiguration\"><Rows><Row><entityType/></Row></Rows></ConfigurationTable></ConfigurationTables><MCPNamespaceReferences><MCPNamespaceReference name=\"N\"><AllowedTools><Tool name=\"keep\"/><Tool name=\"gone\"/></AllowedTools></MCPNamespaceReference></MCPNamespaceReferences></AIAgent></AIAgents></Entities>";
    let (remote, projects) = dropping_an_allowed_tool(&["P"]);
    remote
        .held
        .lock()
        .unwrap()
        .insert(("AIAgents".to_string(), "A".to_string()), exported.to_vec());
    run_test(&remote, &MemoryBaseline::default(), &projects, true, true).unwrap();
    let sent = remote.sent.lock().unwrap();
    let narrowed = super::super::agents::agent(&sent[0].1).unwrap();
    assert_eq!(narrowed.namespaces[0].tools, ["keep"]);
    assert!(String::from_utf8_lossy(&sent[0].1).contains("<entityType/>"));
}

#[test]
fn a_namespace_that_only_adds_narrows_no_agent() {
    let (namespace, agent) = (
        namespace_entity(&["keep", "new"], &[]),
        agent_entity(&["new"]),
    );
    let remote = Fake {
        held: holding(&[namespace_entity(&["keep"], &[]), agent_entity(&["keep"])]),
        ..Fake::default()
    };
    let projects = [project("P", vec![namespace, agent], vec![])];
    let planned = run_test(&remote, &MemoryBaseline::default(), &projects, false, true).unwrap();
    assert!(planned.agents_narrowed.is_empty());
}

/// One building block project whose deploy step is the framework's, as plan_bundles makes it.
fn block_project(config: Value) -> ProjectBundle {
    ProjectBundle {
        deploy: Some(ServiceCall {
            target: "Things/B.EntryPoint".to_string(),
            service: "DeployComponent".to_string(),
            parameters: serde_json::json!({ "deploymentConfig": config }),
        }),
        framework: true,
        ..project("B", vec![target("B.EntryPoint", "x();")], vec![])
    }
}

fn block_server(state: Option<&str>, deploy_result: bool) -> Fake {
    let boolean = |value: bool| serde_json::json!({"rows": [{"result": value}]});
    Fake {
        block_state: Mutex::new(state.map(str::to_string)),
        replies: BTreeMap::from([
            (
                "GetComponentDeployConfigurationParameters".to_string(),
                serde_json::json!({"apiKey": {"types": ["STRING"], "description": "The key", "optional": false}, "units": {"types": ["STRING"], "optional": true}}),
            ),
            (
                "ValidateComponentDeployConfigParams".to_string(),
                boolean(true),
            ),
            ("DeployComponent".to_string(), boolean(deploy_result)),
        ]),
        import_values: BTreeMap::from([(
            "B.xml".to_string(),
            vec![target("B.EntryPoint", "x();")],
        )]),
        ..Fake::default()
    }
}

fn block_run(
    remote: &Fake,
    project: &ProjectBundle,
    apply: bool,
    redeploy: bool,
) -> Result<Report, DeployError> {
    run_with_progress(
        remote,
        &MemoryBaseline::default(),
        &profile(BTreeMap::new()),
        std::slice::from_ref(project),
        RunOptions {
            apply,
            force: true,
            only: false,
            redeploy_blocks: redeploy,
        },
        &super::super::progress::NONE,
    )
}

/// The lifecycle services called; reading and writing the state is not one.
fn services(remote: &Fake) -> Vec<String> {
    remote
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|(_, service, _, _)| service.clone())
        .filter(|service| !service.ends_with("PropertyValues"))
        .collect()
}

#[test]
fn a_block_is_deployed_as_the_framework_deploys_it_and_then_configured() {
    let project = block_project(serde_json::json!({"apiKey": "k"}));
    let remote = block_server(Some("Not Configured"), true);
    let planned = block_run(&remote, &project, false, false).unwrap();
    assert_eq!(
        planned.blocks,
        [BlockStep {
            project: "B".to_string(),
            entry_point: "B.EntryPoint".to_string(),
            state: Some("Not Configured".to_string()),
            runs: true,
            done: false,
        }]
    );
    assert!(
        services(&remote).is_empty(),
        "a plan calls no lifecycle service"
    );

    let report = block_run(&remote, &project, true, false).unwrap();
    assert!(report.blocks[0].done);
    assert_eq!(
        services(&remote),
        [
            "GetComponentDeployConfigurationParameters",
            "ValidateComponentDeployConfigParams",
            "DeployComponent"
        ]
    );
    let calls = remote.calls.lock().unwrap();
    let sent = |service: &str| {
        calls
            .iter()
            .find(|(_, called, _, _)| called == service)
            .map(|(_, _, parameters, _)| parameters.clone())
            .unwrap()
    };
    assert_eq!(
        sent("ValidateComponentDeployConfigParams"),
        serde_json::json!({"jsonParams": {"apiKey": "k"}})
    );
    assert_eq!(
        sent("DeployComponent"),
        serde_json::json!({"deploymentConfig": {"apiKey": "k"}})
    );
    drop(calls);
    assert_eq!(
        remote.block_state.lock().unwrap().as_deref(),
        Some("Configured")
    );
}

#[test]
fn a_configured_block_is_left_unless_redeployed() {
    let project = block_project(serde_json::json!({"apiKey": "k"}));
    let remote = block_server(Some("Configured"), true);
    let report = block_run(&remote, &project, true, false).unwrap();
    assert!(!report.blocks[0].runs && !report.blocks[0].done);
    assert!(services(&remote).is_empty());
    assert!(
        report.calls[0].skipped,
        "the planned DeployComponent call says it did not run"
    );

    let report = block_run(&remote, &project, true, true).unwrap();
    assert!(report.blocks[0].done);
    assert!(!report.calls[0].skipped);
    assert!(services(&remote).contains(&"DeployComponent".to_string()));
}

#[test]
fn a_missing_required_parameter_stops_the_step_before_deploy_component() {
    let project = block_project(serde_json::json!({"units": "metric"}));
    let remote = block_server(Some("Not Configured"), true);
    let error = block_run(&remote, &project, true, false).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("B.EntryPoint.DeployComponent needs deploymentConfig apiKey (The key)"),
        "{error}"
    );
    assert!(!services(&remote).contains(&"DeployComponent".to_string()));
    assert_eq!(
        remote.block_state.lock().unwrap().as_deref(),
        Some("Not Configured")
    );
}

#[test]
fn a_deploy_that_returns_false_keeps_the_state_and_one_that_throws_sets_error() {
    let project = block_project(serde_json::json!({"apiKey": "k"}));
    let remote = block_server(Some("Not Configured"), false);
    let error = block_run(&remote, &project, true, false).unwrap_err();
    assert!(error.to_string().contains("did not return true"), "{error}");
    assert_eq!(
        remote.block_state.lock().unwrap().as_deref(),
        Some("Not Configured")
    );

    let mut remote = block_server(Some("Not Configured"), true);
    remote.fail_service = Some("DeployComponent".to_string());
    let error = block_run(&remote, &project, true, false).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("the entry point's state is Error"),
        "{error}"
    );
    assert_eq!(remote.block_state.lock().unwrap().as_deref(), Some("Error"));
}

#[test]
fn a_new_block_runs_its_step_once_imported() {
    let project = block_project(serde_json::json!({"apiKey": "k"}));
    let remote = block_server(None, true);
    let planned = block_run(&remote, &project, false, false).unwrap();
    assert_eq!(planned.blocks[0].state, None);
    assert!(planned.blocks[0].runs);
}
