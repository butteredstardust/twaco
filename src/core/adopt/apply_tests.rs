use super::compare::compare;
use super::read::replace_entity_bytes;
use super::*;
use crate::core::config::Solution;
use crate::core::lock::WorkspaceLock;
use crate::core::{scan, sidecar, sync};
use std::collections::BTreeMap;
use std::path::Path;

#[test]
fn server_owned_parts_are_neither_reported_nor_adopted_with_other_changes() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("twaco.toml");
    std::fs::write(&config, "[[project]]\nname='Acme.Project'\n[[server_owned]]\nentities=['Groups/Acme.Admin_UG']\nparts=['members','run-time-permissions']\n").unwrap();
    std::fs::create_dir(dir.path().join("Groups")).unwrap();
    let path = dir.path().join("Groups/Acme.Admin_UG.xml");
    let local = "<Entities><Groups><Group name=\"Acme.Admin_UG\" projectName=\"Acme.Project\" description=\"before\"><Members><Member name='Acme.Local'/></Members><RunTimePermissions><Permissions><Read><Principal name='Acme.Local' type='Group'/></Read></Permissions></RunTimePermissions></Group></Groups></Entities>";
    std::fs::write(&path, local).unwrap();
    let solution = Solution::load(&config).unwrap();
    let export = dir.path().join("export.xml");
    let changed_owned = local.replace("Acme.Local", "Acme.Server");
    std::fs::write(&export, &changed_owned).unwrap();
    let report = compare(&solution, &export, &[]).unwrap();
    assert_eq!(report.entities[0].change, Change::Same);
    assert!(report.entities[0].differences.is_empty());
    apply(&solution, &export, &report, &[], &locked(&solution)).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), local);
    std::fs::write(&export, changed_owned.replace("before", "after")).unwrap();
    let report = compare(&solution, &export, &[]).unwrap();
    assert_eq!(report.entities[0].differences.len(), 1);
    assert_eq!(report.entities[0].differences[0].path, "@description");
    apply(
        &solution,
        &export,
        &report,
        &[Take {
            side: TakeSide::Theirs,
            target: "Acme.Admin_UG".into(),
        }],
        &locked(&solution),
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        local.replace("before", "after")
    );
}

#[test]
fn exporter_backend_replacement_keeps_the_repository_lines_and_new_entity_layout() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("twaco.toml"),
        "[[project]]\nname='Acme.Orders'\n",
    )
    .unwrap();
    std::fs::create_dir(dir.path().join("Things")).unwrap();
    let path = dir.path().join("Things/Acme.Orders.Manager.xml");
    let before =
        include_str!("../../../tests/fixtures/corpus/acme-orders/Things/Acme.Orders.Manager.xml");
    std::fs::write(&path, before).unwrap();
    let solution = Solution::load(&dir.path().join("twaco.toml")).unwrap();
    let tokens = scan::tokenize(before.as_bytes()).unwrap();
    let mut flat = String::new();
    for t in &tokens {
        let text = std::str::from_utf8(t.span.of(before.as_bytes())).unwrap();
        match t.kind {
            scan::Kind::Text if text.trim().is_empty() => {}
            scan::Kind::Cdata => flat.push_str(&format!(
                "<![CDATA[{}]]>",
                std::str::from_utf8(t.inner.of(before.as_bytes()))
                    .unwrap()
                    .trim()
            )),
            scan::Kind::Start => {
                flat.push('<');
                flat.push_str(std::str::from_utf8(t.name.of(before.as_bytes())).unwrap());
                for a in scan::attributes(before.as_bytes(), t).unwrap() {
                    flat.push(' ');
                    flat.push_str(&before[a.name.start..a.value.end + 1]);
                }
                flat.push('>');
            }
            _ => flat.push_str(text),
        }
    }
    let export = dir.path().join("export.xml");
    let changed = flat
        .replace("Looks up orders.", "Looks up active orders.")
        .replace("<![CDATA[Orders]]>", "<![CDATA[Active orders]]>")
        .replace("<Thing ", "<Thing lastModifiedDate=\"0\" ")
        .replace("</Thing>", "<Owner>Acme.Exporter</Owner></Thing>");
    std::fs::write(&export, &changed).unwrap();
    let report = compare(&solution, &export, &[]).unwrap();
    let takes = [Take {
        side: TakeSide::Theirs,
        target: "Acme.Orders.Manager".into(),
    }];
    apply(&solution, &export, &report, &takes, &locked(&solution)).unwrap();
    let expected = before
        .replace("Looks up orders.", "Looks up active orders.")
        .replace("<![CDATA[Orders]]>", "<![CDATA[Active orders]]>");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
    for service in sidecar::extract(before.as_bytes()).unwrap().services {
        assert_eq!(
            std::fs::read_to_string(
                solution
                    .src_root()
                    .join("Acme.Orders.Manager/services")
                    .join(&service.name)
                    .join("definition.xml")
            )
            .unwrap(),
            service.definition,
            "incoming service definitions retain the repository layout"
        );
    }
    // A semantically identical flattened export must leave every byte alone.
    let report = compare(&solution, &export, &[]).unwrap();
    apply(&solution, &export, &report, &takes, &locked(&solution)).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
    let new = changed.replace("Acme.Orders.Manager", "Acme.Orders.Worker");
    std::fs::write(&export, new).unwrap();
    let report = compare(&solution, &export, &[]).unwrap();
    apply(&solution, &export, &report, &[], &locked(&solution)).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("Things/Acme.Orders.Worker.xml")).unwrap(),
        expected.replace("Acme.Orders.Manager", "Acme.Orders.Worker")
    );
}

#[test]
fn semantically_unchanged_export_permissions_do_not_rewrite_the_entity() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("twaco.toml"),
        "[[project]]\nname='Acme.App'\n",
    )
    .unwrap();
    std::fs::create_dir(dir.path().join("Things")).unwrap();
    let path = dir.path().join("Things/Acme.Manager.xml");
    let before = "<Entities><Things><Thing name=\"Acme.Manager\" projectName=\"Acme.App\"><RunTimePermissions></RunTimePermissions></Thing></Things></Entities>";
    let theirs = before.replace(
        "<RunTimePermissions></RunTimePermissions>",
        "<RunTimePermissions><Permissions name=\"Acme.Resource\"><Read></Read><Write></Write></Permissions></RunTimePermissions>",
    );
    std::fs::write(&path, before).unwrap();
    let export = dir.path().join("export.xml");
    std::fs::write(&export, theirs).unwrap();
    let solution = Solution::load(&dir.path().join("twaco.toml")).unwrap();
    let report = compare(&solution, &export, &[]).unwrap();
    assert_eq!(report.entities[0].change, Change::Unknown);
    let takes = [Take {
        side: TakeSide::Theirs,
        target: "Acme.Manager".into(),
    }];
    apply(&solution, &export, &report, &takes, &locked(&solution)).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
}

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
fn taking_all_ui_needs_a_ui_conflict_and_the_theirs_side() {
    // A backend-only report: there is no UI conflict for `ui` to name.
    let (_dir, solution, base, export) = backend_case("base();", "ours();", "theirs();");
    let report = compared(&solution, &base, &export);
    let all_ui = |side| {
        [Take {
            side,
            target: TAKE_ALL_UI.to_string(),
        }]
    };
    assert!(matches!(
        apply(
            &solution,
            &export,
            &report,
            &all_ui(TakeSide::Theirs),
            &locked(&solution)
        ),
        Err(AdoptError::Take(_))
    ));
    let (_dir, solution, _base, export) = ui_case(r#"{"x": 1}"#, r#"{"x": 2}"#, r#"{"x": 3}"#);
    let report = compare(&solution, &export, &[]).unwrap();
    let error = apply(
        &solution,
        &export,
        &report,
        &all_ui(TakeSide::Ours),
        &locked(&solution),
    )
    .err()
    .expect("ours:ui is not a take");
    assert!(error.to_string().contains("ui (theirs only)"), "{error}");
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
fn ui_writes_theirs_but_never_stale_unknown_or_unresolved_conflict() {
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

    // No base: an unknown is a conflict, which a take of the mashup or of all UI resolves.
    let (_dir, solution, _base, export) = ui_case(r#"{"x": 1}"#, r#"{"x": 2}"#, r#"{"x": 3}"#);
    let content = solution.root.join("src/P.M/mashup/content.json");
    let report = compare(&solution, &export, &[]).unwrap();
    assert_eq!(report.entities[0].change, Change::Unknown);
    let lines = apply(&solution, &export, &report, &[], &locked(&solution))
        .unwrap()
        .lines;
    assert_eq!(
        lines,
        ["conflict Mashups/P.M: no base shows who changed it; --take theirs:P.M, --take ours:P.M or --take theirs:ui"]
    );
    assert_eq!(std::fs::read_to_string(&content).unwrap(), r#"{"x": 2}"#);
    let all_ui = [Take {
        side: TakeSide::Theirs,
        target: TAKE_ALL_UI.to_string(),
    }];
    apply(&solution, &export, &report, &all_ui, &locked(&solution)).unwrap();
    assert!(std::fs::read_to_string(&content)
        .unwrap()
        .contains("\"x\": 3"));

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
fn an_entity_only_they_changed_keeps_a_sidecar_edit_not_yet_synced() {
    // The entity XML still equals the base, but our sidecar holds newer, unsynced work.
    let (_dir, solution, base, export) = multi_case(
        ("v", &[("A", "a();"), ("B", "b();")]),
        ("v", &[("A", "a();"), ("B", "b();")]),
        ("t", &[("A", "a();"), ("B", "b();")]),
    );
    std::fs::write(sidecar(&solution, "A").join("script.js"), "mine();").unwrap();
    let lines = apply_all(&solution, &base, &export, &[]);
    assert!(
        !lines.iter().any(|l| l.starts_with("replaced")),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|l| l == "kept     P.T.A: stale"),
        "{lines:?}"
    );
    assert_eq!(
        std::fs::read_to_string(sidecar(&solution, "A").join("script.js")).unwrap(),
        "mine();"
    );
    let entity = read(&solution, "Things/P.T.xml");
    assert!(
        entity.contains("<value>t</value>") && entity.contains("mine();"),
        "{entity}"
    );
}

#[test]
fn a_mashup_only_they_changed_keeps_a_sidecar_edit_not_yet_synced() {
    let (_dir, solution, base, export) = ui_case(r#"{"x": 1}"#, r#"{"x": 1}"#, r#"{"x": 2}"#);
    let content = solution.root.join("src/P.M/mashup/content.json");
    std::fs::write(&content, r#"{"x": 5}"#).unwrap();
    let report = compared(&solution, &base, &export);
    let lines = apply(&solution, &export, &report, &[], &locked(&solution))
        .unwrap()
        .lines;
    assert_eq!(
        lines,
        ["kept     Mashups/P.M: its sidecars changed since the last sync; sync, then adopt again"]
    );
    assert_eq!(std::fs::read_to_string(&content).unwrap(), r#"{"x": 5}"#);
    // An explicit take still wins.
    let takes = [Take {
        side: TakeSide::Theirs,
        target: "P.M".to_string(),
    }];
    let (_dir, solution, base, export) = ui_case(r#"{"x": 1}"#, r#"{"x": 2}"#, r#"{"x": 3}"#);
    let content = solution.root.join("src/P.M/mashup/content.json");
    std::fs::write(&content, r#"{"x": 5}"#).unwrap();
    let report = compared(&solution, &base, &export);
    assert_eq!(report.entities[0].change, Change::Conflict);
    apply(&solution, &export, &report, &[], &locked(&solution)).unwrap();
    assert_eq!(std::fs::read_to_string(&content).unwrap(), r#"{"x": 5}"#);
    apply(&solution, &export, &report, &takes, &locked(&solution)).unwrap();
    assert!(std::fs::read_to_string(&content)
        .unwrap()
        .contains("\"x\": 3"));
}

#[test]
fn taking_our_entity_keeps_it_whole_and_quiet() {
    let (_dir, solution, base, export) = multi_case(
        ("v", &[("A", "a();")]),
        ("o", &[("A", "a();")]),
        ("t", &[("A", "a2();")]),
    );
    let before = file_snapshot(&solution.root);
    let lines = apply_all(
        &solution,
        &base,
        &export,
        &[Take {
            side: TakeSide::Ours,
            target: "P.T".to_string(),
        }],
    );
    assert_eq!(lines, ["kept     Things/P.T: ours"]);
    assert_eq!(file_snapshot(&solution.root), before);

    let (_dir, solution, base, export) = ui_case(r#"{"x": 1}"#, r#"{"x": 2}"#, r#"{"x": 3}"#);
    let report = compared(&solution, &base, &export);
    let takes = [Take {
        side: TakeSide::Ours,
        target: "P.M".to_string(),
    }];
    let lines = apply(&solution, &export, &report, &takes, &locked(&solution))
        .unwrap()
        .lines;
    assert_eq!(lines, ["kept     Mashups/P.M: ours"]);
}

#[test]
fn a_changed_crlf_sidecar_keeps_crlf() {
    let (_dir, solution, base, export) = multi_case(
        ("v", &[("A", "a();")]),
        ("v", &[("A", "a();")]),
        ("v", &[("A", "a2();\nz();")]),
    );
    let script = sidecar(&solution, "A").join("script.js");
    std::fs::write(&script, "a();\r\n").unwrap();
    apply_all(&solution, &base, &export, &[]);
    assert_eq!(std::fs::read_to_string(&script).unwrap(), "a2();\r\nz();");
}

#[test]
fn backend_names_that_cannot_be_file_names_are_refused_before_any_write() {
    for (entity, service) in [("../P.Evil", "Run"), ("P.New", "../Run"), ("P.New", "CON")] {
        let (_dir, solution, base, export) = multi_case(
            ("v", &[("A", "a();")]),
            ("v", &[("A", "a();")]),
            ("v", &[("A", "a();")]),
        );
        let new = entity_named(entity, "n", &[(service, "run();")]);
        let xml = std::fs::read_to_string(&export)
            .unwrap()
            .replace("</Things></Entities>", &new["<Entities><Things>".len()..]);
        std::fs::write(&export, xml).unwrap();
        let before = file_snapshot(&solution.root);
        let report = compared(&solution, &base, &export);
        let error = apply(&solution, &export, &report, &[], &locked(&solution))
            .err()
            .unwrap_or_else(|| panic!("{entity}.{service} was adopted"));
        assert!(
            error.to_string().contains("cannot be a file name"),
            "{entity}.{service}: {error}"
        );
        assert_eq!(file_snapshot(&solution.root), before, "{entity}.{service}");
    }
}

#[test]
fn a_ui_entity_adopt_cannot_write_says_so() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
    let export = dir.path().join("export.xml");
    std::fs::write(
        &export,
        "<Entities><StyleThemes><StyleTheme name=\"P.Theme\" projectName=\"P\"/></StyleThemes></Entities>",
    )
    .unwrap();
    let solution = Solution::load(&dir.path().join("twaco.toml")).unwrap();
    let report = compare(&solution, &export, &[]).unwrap();
    let lines = apply(&solution, &export, &report, &[], &locked(&solution))
        .unwrap()
        .lines;
    assert_eq!(
        lines,
        ["not adopted StyleThemes/P.Theme: adopt writes mashups and media only; import it from the export"]
    );
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
#[test]
fn adopt_keeps_equivalent_nodes_verbatim_around_one_real_attribute_change() {
    let ours = r#"<Entities><MediaEntities><MediaEntity
 name="Acme.Asset"
 description="old">
    <VisibilityPermissions>
        <Visibility><Principal name="Acme.Users" type="Group"/><Principal name="Acme.Users" type="Organization"/></Visibility>
    </VisibilityPermissions>
    <ServiceDefinitions>
        <ServiceDefinition name="Acme.First"/>
        <ServiceDefinition name="Acme.Second"/>
    </ServiceDefinitions>
    <content><![CDATA[YWJj
ZGVm]]></content>
    <mashupContent><![CDATA[{
  "first" : 1,
  "second" : { "enabled": true }
}]]></mashupContent>
    <code><![CDATA[var result = true;]]></code>
</MediaEntity></MediaEntities></Entities>"#;
    let theirs = br#"<MediaEntity description="new" name="Acme.Asset"><VisibilityPermissions><Visibility><Principal type="Organization" name="Acme.Users"/><Principal type="Group" name="Acme.Users"/></Visibility></VisibilityPermissions><ServiceDefinitions><ServiceDefinition name="Acme.Second"/><ServiceDefinition name="Acme.First"/></ServiceDefinitions><content><![CDATA[YWJjZGVm]]></content><mashupContent><![CDATA[{"second":{"enabled":true},"first":1}]]></mashupContent><code><![CDATA[
var result = true;
]]></code><Owner>Acme.Exporter</Owner></MediaEntity>"#;
    let wanted = EntityRef {
        collection: "MediaEntities".into(),
        name: "Acme.Asset".into(),
    };
    let out = replace_entity_bytes(ours.as_bytes(), &wanted, theirs, false).unwrap();
    assert_eq!(
        out,
        ours.replace("description=\"old\"", "description=\"new\"")
            .as_bytes()
    );
}

#[test]
fn a_new_entity_without_a_peer_file_gets_the_source_control_envelope() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("twaco.toml"),
        "[[project]]\nname='Acme.App'\n",
    )
    .unwrap();
    std::fs::create_dir(dir.path().join("Things")).unwrap();
    std::fs::write(
        dir.path().join("Things/Acme.Manager.xml"),
        "<Entities><Things><Thing name=\"Acme.Manager\" projectName=\"Acme.App\"></Thing></Things></Entities>",
    )
    .unwrap();
    let export = dir.path().join("export.xml");
    std::fs::write(
        &export,
        "<Entities><DataShapes><DataShape lastModifiedDate=\"0\" name=\"Acme.Row_DS\" projectName=\"Acme.App\"><FieldDefinitions/></DataShape></DataShapes></Entities>",
    )
    .unwrap();
    let solution = Solution::load(&dir.path().join("twaco.toml")).unwrap();
    let report = compare(&solution, &export, &[]).unwrap();
    apply(&solution, &export, &report, &[], &locked(&solution)).unwrap();
    let written = std::fs::read_to_string(dir.path().join("DataShapes/Acme.Row_DS.xml")).unwrap();
    assert_eq!(
        written,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Entities\n majorVersion=\"10\"\n minorVersion=\"1\"\n universal=\"password\">\n    <DataShapes>\n        <DataShape\n         name=\"Acme.Row_DS\"\n         projectName=\"Acme.App\">\n            <FieldDefinitions></FieldDefinitions>\n        </DataShape>\n    </DataShapes>\n</Entities>\n"
    );
}

fn data_shape(description: &str) -> String {
    format!(
        "<Entities><DataShapes><DataShape name=\"P.D\" projectName=\"P\"><FieldDefinitions>\
         <FieldDefinition baseType=\"STRING\" description=\"{description}\" name=\"a\" ordinal=\"1\"/>\
         </FieldDefinitions></DataShape></DataShapes></Entities>"
    )
}

fn data_shape_case(fields: &str) -> (tempfile::TempDir, Solution, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
    std::fs::create_dir(dir.path().join("DataShapes")).unwrap();
    std::fs::write(dir.path().join("DataShapes/P.D.xml"), data_shape("old")).unwrap();
    std::fs::create_dir_all(dir.path().join("src/P.D")).unwrap();
    let synced = crate::core::datashape::extract(data_shape(fields).as_bytes()).unwrap();
    std::fs::write(
        dir.path().join("src/P.D/fields.json"),
        crate::core::datashape::to_sidecar(&synced),
    )
    .unwrap();
    let base_file = dir.path().join("base.xml");
    std::fs::write(&base_file, data_shape("old")).unwrap();
    let export = dir.path().join("export.xml");
    std::fs::write(&export, data_shape("new")).unwrap();
    let solution = Solution::load(&dir.path().join("twaco.toml")).unwrap();
    (dir, solution, base_file, export)
}

/// `sync` reads fields.json as the source of truth: one left stale would put the old field back.
#[test]
fn an_adopted_data_shape_brings_its_fields_json_along() {
    let (_dir, solution, base, export) = data_shape_case("old");
    let lines = apply_all(&solution, &base, &export, &[]);
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("sidecar") && l.contains("fields.json")),
        "{lines:?}"
    );
    assert!(read(&solution, "DataShapes/P.D.xml").contains("description=\"new\""));
    let fields = read(&solution, "src/P.D/fields.json");
    assert!(
        fields.contains("\"new\"") && !fields.contains("\"old\""),
        "{fields}"
    );
    // Nothing left for a sync to undo, and a second apply writes nothing.
    assert!(apply_all(&solution, &base, &export, &[])
        .iter()
        .all(|l| !l.starts_with("sidecar")));
}

#[test]
fn a_fields_json_edited_since_the_sync_is_never_overwritten() {
    let (_dir, solution, base, export) = data_shape_case("mine");
    let before = file_snapshot(&solution.root);
    let report = compared(&solution, &base, &export);
    let lock = locked(&solution);
    let Err(error) = apply(&solution, &export, &report, &[], &lock) else {
        panic!("the apply went ahead");
    };
    assert!(
        error.to_string().contains("changed since the last sync"),
        "{error}"
    );
    drop(lock);
    assert_eq!(file_snapshot(&solution.root), before, "nothing was adopted");
}

#[test]
fn a_data_shape_they_added_comes_with_its_fields_json() {
    let (_dir, solution, base, export) = data_shape_case("old");
    let both = data_shape("old").replace(
        "</DataShape>",
        "</DataShape><DataShape name=\"P.New\" projectName=\"P\"><FieldDefinitions>\
         <FieldDefinition baseType=\"NUMBER\" description=\"\" name=\"n\" ordinal=\"1\"/>\
         </FieldDefinitions></DataShape>",
    );
    std::fs::write(&export, &both).unwrap();
    let lines = apply_all(&solution, &base, &export, &[]);
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("created  DataShapes/P.New.xml")),
        "{lines:?}"
    );
    let fields = read(&solution, "src/P.New/fields.json");
    assert!(fields.contains("\"NUMBER\""), "{fields}");
    // The shape already here was not changed, so its sidecar is left alone.
    assert!(read(&solution, "src/P.D/fields.json").contains("\"old\""));
}
