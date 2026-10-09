use super::*;

fn simple_entity(value: &str, script: &str) -> String {
    format!(
        "<Entities><Things><Thing name=\"P.T\" projectName=\"P\"><value>{value}</value><ThingShape>\
         <ServiceDefinitions><ServiceDefinition name=\"S\"><ResultType baseType=\"NOTHING\"/></ServiceDefinition></ServiceDefinitions>\
         <ServiceImplementations><ServiceImplementation name=\"S\" handlerName=\"Script\"><ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row><code><![CDATA[{script}]]></code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations>\
         </ThingShape></Thing></Things></Entities>"
    )
}

fn backend_case(
    base: &str,
    ours: &str,
    theirs: &str,
) -> (tempfile::TempDir, Solution, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
    std::fs::create_dir(dir.path().join("Things")).unwrap();
    std::fs::write(dir.path().join("Things/P.T.xml"), simple_entity("v", ours)).unwrap();
    std::fs::create_dir_all(dir.path().join("src/P.T/services/S")).unwrap();
    std::fs::write(dir.path().join("src/P.T/services/S/script.js"), ours).unwrap();
    std::fs::write(
        dir.path().join("src/P.T/services/S/definition.xml"),
        "<ServiceDefinition name=\"S\"><ResultType baseType=\"NOTHING\"/></ServiceDefinition>\n",
    )
    .unwrap();
    let base_file = dir.path().join("base.xml");
    std::fs::write(&base_file, simple_entity("v", base)).unwrap();
    let export = dir.path().join("export.xml");
    std::fs::write(&export, simple_entity("v", theirs)).unwrap();
    let solution = Solution::load(&dir.path().join("twaco.toml")).unwrap();
    (dir, solution, base_file, export)
}

fn compared(solution: &Solution, base: &Path, export: &Path) -> Report {
    compare_with(
        solution,
        export,
        &[],
        &CompareOptions {
            base: Some(base.display().to_string()),
            only_kind: None,
        },
    )
    .unwrap()
}

fn locked(solution: &Solution) -> WorkspaceLock {
    crate::core::lock::acquire(&solution.root, "adopt apply test", &[]).unwrap()
}

fn ui_case(
    base: &str,
    ours: &str,
    theirs: &str,
) -> (tempfile::TempDir, Solution, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
    std::fs::create_dir(dir.path().join("Mashups")).unwrap();
    let mashup = |content: &str| {
        format!("<Entities><Mashups><Mashup name=\"P.M\" projectName=\"P\"><mashupContent><![CDATA[{content}]]></mashupContent></Mashup></Mashups></Entities>")
    };
    std::fs::write(dir.path().join("Mashups/P.M.xml"), mashup(ours)).unwrap();
    std::fs::create_dir_all(dir.path().join("src/P.M/mashup")).unwrap();
    std::fs::write(dir.path().join("src/P.M/mashup/content.json"), ours).unwrap();
    std::fs::write(dir.path().join("src/P.M/mashup/custom.css"), "").unwrap();
    let base_file = dir.path().join("base.xml");
    std::fs::write(&base_file, mashup(base)).unwrap();
    let export = dir.path().join("export.xml");
    std::fs::write(&export, mashup(theirs)).unwrap();
    let solution = Solution::load(&dir.path().join("twaco.toml")).unwrap();
    (dir, solution, base_file, export)
}

#[test]
fn backend_theirs_updates_only_the_service_spans_and_sidecar() {
    let (_dir, solution, base, export) = backend_case("old();", "old();", "theirs();");
    let before = std::fs::read(solution.root.join("Things/P.T.xml")).unwrap();
    let report = compared(&solution, &base, &export);
    apply(&solution, &export, &report, &[], &locked(&solution)).unwrap();
    let after = std::fs::read(solution.root.join("Things/P.T.xml")).unwrap();
    assert!(String::from_utf8_lossy(&after).contains("theirs();"));
    assert_eq!(
        std::fs::read_to_string(solution.root.join("src/P.T/services/S/script.js")).unwrap(),
        "theirs();"
    );
    let old = b"old();";
    let new = b"theirs();";
    let at = before
        .windows(old.len())
        .position(|window| window == old)
        .unwrap();
    assert_eq!(&before[..at], &after[..at]);
    assert_eq!(&after[at..at + new.len()], new);
    assert_eq!(&before[at + old.len()..], &after[at + new.len()..]);
}

#[test]
fn stale_backend_service_is_a_noop_and_conflict_needs_a_take() {
    let (_dir, solution, base, export) = backend_case("base();", "ours();", "base();");
    let before = std::fs::read(solution.root.join("Things/P.T.xml")).unwrap();
    let report = compared(&solution, &base, &export);
    let outcome = apply(&solution, &export, &report, &[], &locked(&solution)).unwrap();
    assert_eq!(
        std::fs::read(solution.root.join("Things/P.T.xml")).unwrap(),
        before
    );
    assert!(outcome.lines.iter().any(|line| line.contains("stale")));

    let (_dir, solution, base, export) = backend_case("base();", "ours();", "theirs();");
    let report = compared(&solution, &base, &export);
    apply(&solution, &export, &report, &[], &locked(&solution)).unwrap();
    assert_eq!(
        std::fs::read_to_string(solution.root.join("src/P.T/services/S/script.js")).unwrap(),
        "ours();"
    );
    let takes = [Take {
        side: TakeSide::Theirs,
        target: "P.T.S".to_string(),
    }];
    apply(&solution, &export, &report, &takes, &locked(&solution)).unwrap();
    assert_eq!(
        std::fs::read_to_string(solution.root.join("src/P.T/services/S/script.js")).unwrap(),
        "theirs();"
    );
}

#[test]
fn an_unknown_take_is_refused_before_any_write() {
    let (_dir, solution, base, export) = backend_case("old();", "old();", "theirs();");
    let before = std::fs::read(solution.root.join("Things/P.T.xml")).unwrap();
    let report = compared(&solution, &base, &export);
    let takes = [Take {
        side: TakeSide::Theirs,
        target: "P.T.Missing".to_string(),
    }];
    assert!(matches!(
        apply(&solution, &export, &report, &takes, &locked(&solution)),
        Err(AdoptError::Take(_))
    ));
    assert_eq!(
        std::fs::read(solution.root.join("Things/P.T.xml")).unwrap(),
        before
    );
}

#[test]
fn ui_writes_theirs_and_unknown_without_a_base_but_never_stale_or_unresolved_conflict() {
    let (_dir, solution, base, export) = ui_case(r#"{"x": 1}"#, r#"{"x": 1}"#, r#"{"x": 2}"#);
    let report = compared(&solution, &base, &export);
    apply(&solution, &export, &report, &[], &locked(&solution)).unwrap();
    assert!(
        std::fs::read_to_string(solution.root.join("src/P.M/mashup/content.json"))
            .unwrap()
            .contains("\"x\": 2")
    );

    let (_dir, solution, base, export) = ui_case(r#"{"x": 1}"#, r#"{"x": 2}"#, r#"{"x": 1}"#);
    let report = compared(&solution, &base, &export);
    apply(&solution, &export, &report, &[], &locked(&solution)).unwrap();
    assert_eq!(
        std::fs::read_to_string(solution.root.join("src/P.M/mashup/content.json")).unwrap(),
        r#"{"x": 2}"#
    );

    let (_dir, solution, _base, export) = ui_case(r#"{"x": 1}"#, r#"{"x": 2}"#, r#"{"x": 3}"#);
    let report = compare(&solution, &export, &[]).unwrap();
    apply(&solution, &export, &report, &[], &locked(&solution)).unwrap();
    assert!(
        std::fs::read_to_string(solution.root.join("src/P.M/mashup/content.json"))
            .unwrap()
            .contains("\"x\": 3")
    );

    let (_dir, solution, base, export) = ui_case(r#"{"x": 1}"#, r#"{"x": 2}"#, r#"{"x": 3}"#);
    let report = compared(&solution, &base, &export);
    apply(&solution, &export, &report, &[], &locked(&solution)).unwrap();
    assert_eq!(
        std::fs::read_to_string(solution.root.join("src/P.M/mashup/content.json")).unwrap(),
        r#"{"x": 2}"#
    );
    let takes = [Take {
        side: TakeSide::Theirs,
        target: "P.M".to_string(),
    }];
    apply(&solution, &export, &report, &takes, &locked(&solution)).unwrap();
    assert!(
        std::fs::read_to_string(solution.root.join("src/P.M/mashup/content.json"))
            .unwrap()
            .contains("\"x\": 3")
    );
}

#[test]
fn applying_twice_writes_nothing_the_second_time() {
    let (_dir, solution, base, export) = backend_case("old();", "old();", "theirs();");
    let report = compared(&solution, &base, &export);
    apply(&solution, &export, &report, &[], &locked(&solution)).unwrap();
    let again = compared(&solution, &base, &export);
    let before = std::fs::read(solution.root.join("Things/P.T.xml")).unwrap();
    let outcome = apply(&solution, &export, &again, &[], &locked(&solution)).unwrap();
    assert!(outcome
        .lines
        .iter()
        .all(|line| !line.starts_with("replaced")
            && !line.starts_with("merged")
            && !line.starts_with("sidecar")));
    assert_eq!(
        std::fs::read(solution.root.join("Things/P.T.xml")).unwrap(),
        before
    );
}

// ---------- entities with several services ----------

/// A Thing with a frame `value` and the named script services.
fn entity_with(value: &str, services: &[(&str, &str)]) -> String {
    entity_named("P.T", value, services)
}

fn entity_named(name: &str, value: &str, services: &[(&str, &str)]) -> String {
    let definitions: String = services
        .iter()
        .map(|(service, _)| {
            format!("<ServiceDefinition name=\"{service}\"><ResultType baseType=\"NOTHING\"/></ServiceDefinition>")
        })
        .collect();
    let implementations: String = services
        .iter()
        .map(|(service, script)| {
            format!("<ServiceImplementation name=\"{service}\" handlerName=\"Script\"><ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row><code><![CDATA[{script}]]></code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation>")
        })
        .collect();
    format!(
        "<Entities><Things><Thing name=\"{name}\" projectName=\"P\"><value>{value}</value><ThingShape>\
         <ServiceDefinitions>{definitions}</ServiceDefinitions>\
         <ServiceImplementations>{implementations}</ServiceImplementations>\
         </ThingShape></Thing></Things></Entities>"
    )
}

/// A solution whose `Things/P.T.xml` and sidecars hold `ours`, with a base and an export.
fn multi_case(
    base: (&str, &[(&str, &str)]),
    ours: (&str, &[(&str, &str)]),
    theirs: (&str, &[(&str, &str)]),
) -> (tempfile::TempDir, Solution, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
    std::fs::create_dir(dir.path().join("Things")).unwrap();
    std::fs::write(
        dir.path().join("Things/P.T.xml"),
        entity_with(ours.0, ours.1),
    )
    .unwrap();
    for (service, script) in ours.1 {
        let folder = dir.path().join("src/P.T/services").join(service);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("script.js"), script).unwrap();
        std::fs::write(
            folder.join("definition.xml"),
            format!("<ServiceDefinition name=\"{service}\"><ResultType baseType=\"NOTHING\"/></ServiceDefinition>\n"),
        )
        .unwrap();
    }
    let base_file = dir.path().join("base.xml");
    std::fs::write(&base_file, entity_with(base.0, base.1)).unwrap();
    let export = dir.path().join("export.xml");
    std::fs::write(&export, entity_with(theirs.0, theirs.1)).unwrap();
    let solution = Solution::load(&dir.path().join("twaco.toml")).unwrap();
    (dir, solution, base_file, export)
}

fn apply_all(solution: &Solution, base: &Path, export: &Path, takes: &[Take]) -> Vec<String> {
    let report = compared(solution, base, export);
    let lock = locked(solution);
    apply(solution, export, &report, takes, &lock)
        .unwrap()
        .lines
}

fn read(solution: &Solution, relative: &str) -> String {
    std::fs::read_to_string(solution.root.join(relative)).unwrap()
}

fn sidecar(solution: &Solution, service: &str) -> PathBuf {
    solution.root.join("src/P.T/services").join(service)
}

fn file_snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.ends_with(".twaco") {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else {
                out.insert(path.clone(), std::fs::read(path).unwrap());
            }
        }
    }
    out
}

#[test]
fn a_service_they_added_becomes_a_sidecar_and_a_service_of_the_entity() {
    let (_dir, solution, base, export) = multi_case(
        ("v", &[("A", "a();")]),
        ("v", &[("A", "a();")]),
        ("v", &[("A", "a();"), ("B", "b();")]),
    );
    apply_all(&solution, &base, &export, &[]);
    assert_eq!(
        std::fs::read_to_string(sidecar(&solution, "B").join("script.js")).unwrap(),
        "b();"
    );
    assert!(sidecar(&solution, "B").join("definition.xml").is_file());
    let entity = read(&solution, "Things/P.T.xml");
    assert!(entity.contains("ServiceDefinition name=\"B\""), "{entity}");
    assert!(
        entity.contains("ServiceImplementation name=\"B\""),
        "{entity}"
    );
    // The result is in step with its sidecars: a sync changes nothing.
    let sidecars: BTreeMap<String, sidecar::ServiceSidecar> = sidecar::extract(entity.as_bytes())
        .unwrap()
        .services
        .into_iter()
        .map(|s| (s.name.clone(), s))
        .collect();
    let (again, _) = sync::sync(entity.as_bytes(), &sidecars, false, false, false).unwrap();
    assert_eq!(again, entity.as_bytes());
}

#[test]
fn a_service_we_removed_is_not_brought_back() {
    let (_dir, solution, base, export) = multi_case(
        ("v", &[("A", "a();"), ("B", "b();")]),
        ("v", &[("A", "a();")]),
        ("v", &[("A", "a();"), ("B", "b2();")]),
    );
    let before = file_snapshot(&solution.root);
    apply_all(&solution, &base, &export, &[]);
    assert_eq!(file_snapshot(&solution.root), before);
    assert!(!sidecar(&solution, "B").exists());
}

#[test]
fn an_entity_they_added_is_written_from_the_export_with_its_sidecars() {
    let (_dir, solution, base, export) = multi_case(
        ("v", &[("A", "a();")]),
        ("v", &[("A", "a();")]),
        ("v", &[("A", "a();")]),
    );
    let new = entity_named("P.New", "n", &[("Run", "run();")]);
    let both = format!(
        "{}{}",
        &entity_with("v", &[("A", "a();")])
            [..entity_with("v", &[("A", "a();")]).len() - "</Things></Entities>".len()],
        &new["<Entities><Things>".len()..]
    );
    std::fs::write(&export, &both).unwrap();
    let lines = apply_all(&solution, &base, &export, &[]);
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("created  Things/P.New.xml")),
        "{lines:?}"
    );
    let written = read(&solution, "Things/P.New.xml");
    let entity_bytes = &new["<Entities><Things>".len()..new.len() - "</Things></Entities>".len()];
    assert!(written.contains(entity_bytes), "{written}");
    assert_eq!(
        std::fs::read_to_string(solution.root.join("src/P.New/services/Run/script.js")).unwrap(),
        "run();"
    );
}

#[test]
fn an_entity_only_they_changed_is_replaced_and_a_dropped_service_loses_its_sidecar() {
    let (_dir, solution, base, export) = multi_case(
        ("v", &[("A", "a();"), ("B", "b();")]),
        ("v", &[("A", "a();"), ("B", "b();")]),
        ("t", &[("A", "a2();")]),
    );
    let lines = apply_all(&solution, &base, &export, &[]);
    assert!(lines.iter().any(|l| l.starts_with("replaced")), "{lines:?}");
    let entity = read(&solution, "Things/P.T.xml");
    assert!(
        entity.contains("<value>t</value>") && entity.contains("a2();"),
        "{entity}"
    );
    assert!(!entity.contains("name=\"B\""), "{entity}");
    assert!(!sidecar(&solution, "B").join("script.js").exists());
    assert_eq!(
        std::fs::read_to_string(sidecar(&solution, "A").join("script.js")).unwrap(),
        "a2();"
    );
}

#[test]
fn both_changed_merges_their_frame_and_service_with_our_newer_service() {
    // Their frame and service A changed; our service B changed since the base.
    let (_dir, solution, base, export) = multi_case(
        ("v", &[("A", "a();"), ("B", "b();")]),
        ("v", &[("A", "a();"), ("B", "b2();")]),
        ("t", &[("A", "a2();"), ("B", "b();")]),
    );
    let lines = apply_all(&solution, &base, &export, &[]);
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("merged   P.T: took 1 services, kept 1 (stale), frame theirs")),
        "{lines:?}"
    );
    let entity = read(&solution, "Things/P.T.xml");
    assert!(entity.contains("<value>t</value>"), "{entity}");
    assert!(
        entity.contains("a2();") && entity.contains("b2();"),
        "{entity}"
    );
    assert_eq!(
        std::fs::read_to_string(sidecar(&solution, "B").join("script.js")).unwrap(),
        "b2();"
    );
    assert_eq!(
        std::fs::read_to_string(sidecar(&solution, "A").join("script.js")).unwrap(),
        "a2();"
    );
}

#[test]
fn a_frame_both_changed_keeps_ours_and_says_so() {
    let (_dir, solution, base, export) = multi_case(
        ("v", &[("A", "a();")]),
        ("o", &[("A", "a();")]),
        ("t", &[("A", "a2();")]),
    );
    let lines = apply_all(&solution, &base, &export, &[]);
    assert!(
        lines
            .iter()
            .any(|l| l.contains("frame ours; frame conflict")),
        "{lines:?}"
    );
    let entity = read(&solution, "Things/P.T.xml");
    assert!(
        entity.contains("<value>o</value>") && entity.contains("a2();"),
        "{entity}"
    );
    // A take of the entity uses their frame.
    let (_dir, solution, base, export) = multi_case(
        ("v", &[("A", "a();")]),
        ("o", &[("A", "a();")]),
        ("t", &[("A", "a2();")]),
    );
    apply_all(
        &solution,
        &base,
        &export,
        &[Take {
            side: TakeSide::Theirs,
            target: "P.T".to_string(),
        }],
    );
    assert!(read(&solution, "Things/P.T.xml").contains("<value>t</value>"));
}

#[test]
fn a_kept_sidecar_keeps_its_crlf_bytes() {
    let (_dir, solution, base, export) = multi_case(
        ("v", &[("A", "a();"), ("B", "b();")]),
        ("v", &[("A", "a();"), ("B", "b();")]),
        ("v", &[("A", "a2();"), ("B", "b();")]),
    );
    let definition = sidecar(&solution, "B").join("definition.xml");
    let crlf = std::fs::read_to_string(&definition)
        .unwrap()
        .replace('\n', "\r\n");
    std::fs::write(&definition, &crlf).unwrap();
    apply_all(&solution, &base, &export, &[]);
    assert_eq!(std::fs::read_to_string(&definition).unwrap(), crlf);
}

#[test]
fn a_failure_preparing_a_later_entity_writes_nothing() {
    let (_dir, solution, base, export) = multi_case(
        ("v", &[("A", "a();")]),
        ("v", &[("A", "a();")]),
        ("v", &[("A", "a2();")]),
    );
    // A mashup whose name cannot be a file name fails the preparation after P.T was planned.
    let xml = std::fs::read_to_string(&export).unwrap().replace(
        "</Entities>",
        "<Mashups><Mashup name=\"bad/name\" projectName=\"P\"/></Mashups></Entities>",
    );
    std::fs::write(&export, xml).unwrap();
    let before = file_snapshot(&solution.root);
    let report = compared(&solution, &base, &export);
    let lock = locked(&solution);
    assert!(apply(&solution, &export, &report, &[], &lock).is_err());
    assert_eq!(file_snapshot(&solution.root), before);
}
