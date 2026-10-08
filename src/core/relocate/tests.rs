use super::*;
use crate::core::workflow;
use std::collections::BTreeMap;

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    solution: Solution,
}

fn write(root: &Path, relative: &str, text: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.insert(path.clone(), std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, &mut out);
    out
}

const SCRIPT: &str = "            logger.info(\"hello \" + who);\n            var kept = {\n                a: 1\n            };\n";

fn alpha_definition(indent: &str) -> String {
    let lines = [
        "<ServiceDefinition name=\"Alpha\">",
        "    <ResultType baseType=\"NOTHING\" name=\"result\"></ResultType>",
        "    <ParameterDefinitions>",
        "        <FieldDefinition baseType=\"STRING\" name=\"who\"></FieldDefinition>",
        "    </ParameterDefinitions>",
        "</ServiceDefinition>",
    ];
    lines
        .iter()
        .map(|line| format!("{indent}{line}\n"))
        .collect()
}

fn alpha_implementation(indent: &str) -> String {
    format!(
        "{indent}<ServiceImplementation handlerName=\"Script\" name=\"Alpha\">\n{indent}    <ConfigurationTables>\n{indent}        <ConfigurationTable name=\"Script\">\n{indent}            <Rows>\n{indent}                <Row>\n{indent}                    <code><![CDATA[\n{SCRIPT}{indent}                    ]]></code>\n{indent}                </Row>\n{indent}            </Rows>\n{indent}        </ConfigurationTable>\n{indent}    </ConfigurationTables>\n{indent}</ServiceImplementation>\n"
    )
}

fn shape(name: &str, properties: &str, definitions: &str, implementations: Option<&str>) -> String {
    let implementations = implementations
        .map(|text| format!("            {text}\n"))
        .unwrap_or_default();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Entities>\n    <ThingShapes>\n        <ThingShape name=\"{name}\" projectName=\"P\">\n            {properties}\n            {definitions}\n{implementations}        </ThingShape>\n    </ThingShapes>\n</Entities>\n"
    )
}

fn thing(name: &str, shapes: &[&str], members: &str) -> String {
    let implemented: String = shapes
        .iter()
        .map(|shape| {
            format!("            <ImplementedShape name=\"{shape}\"></ImplementedShape>\n")
        })
        .collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Entities>\n    <Things>\n        <Thing name=\"{name}\" projectName=\"P\" thingTemplate=\"GenericThing\">\n            <ImplementedShapes>\n{implemented}            </ImplementedShapes>\n            <ThingShape>\n{members}            </ThingShape>\n        </Thing>\n    </Things>\n</Entities>\n"
    )
}

/// Base shape with Alpha and Level; an empty target shape; an instance that implements Base; and a
/// caller of the instance. Sidecars are extracted, so everything starts in step.
fn locked(fixture: &Fixture) -> crate::core::lock::WorkspaceLock {
    crate::core::lock::acquire(&fixture.root, "test", &[]).unwrap()
}

fn fixture() -> Fixture {
    let root_guard = tempfile::Builder::new()
        .prefix("twaco-relocate-")
        .tempdir()
        .unwrap();
    let root = root_guard.path().to_path_buf();
    std::fs::create_dir_all(&root).unwrap();
    write(&root, "twaco.toml", "[[project]]\nname = \"P\"\ncollections = [\"Things\", \"ThingShapes\", \"ThingTemplates\"]\n");
    let base = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Entities>\n    <ThingShapes>\n        <ThingShape name=\"P.Base_TS\" projectName=\"P\">\n            <PropertyDefinitions>\n                <PropertyDefinition baseType=\"NUMBER\" name=\"Level\"></PropertyDefinition>\n            </PropertyDefinitions>\n            <ServiceDefinitions>\n{}            </ServiceDefinitions>\n            <ServiceImplementations>\n{}            </ServiceImplementations>\n        </ThingShape>\n    </ThingShapes>\n</Entities>\n",
        alpha_definition("                "),
        alpha_implementation("                ")
    );
    write(&root, "ThingShapes/P.Base_TS.xml", &base);
    write(
        &root,
        "ThingShapes/P.Target_TS.xml",
        &shape(
            "P.Target_TS",
            "<PropertyDefinitions></PropertyDefinitions>",
            "<ServiceDefinitions></ServiceDefinitions>",
            Some("<ServiceImplementations></ServiceImplementations>"),
        ),
    );
    let members = "                <PropertyDefinitions></PropertyDefinitions>\n                <ServiceDefinitions></ServiceDefinitions>\n                <ServiceImplementations></ServiceImplementations>\n";
    write(
        &root,
        "Things/P.Inst.xml",
        &thing("P.Inst", &["P.Base_TS"], members),
    );
    let caller_members =
        "                <ServiceDefinitions>\n                    <ServiceDefinition name=\"Use\">\n                        <ResultType baseType=\"NOTHING\" name=\"result\"></ResultType>\n                    </ServiceDefinition>\n                </ServiceDefinitions>\n                <ServiceImplementations>\n                    <ServiceImplementation handlerName=\"Script\" name=\"Use\">\n                        <ConfigurationTables>\n                            <ConfigurationTable name=\"Script\">\n                                <Rows>\n                                    <Row>\n                                        <code><![CDATA[\nThings[\"P.Inst\"].Alpha({ who: \"x\" });\n                                        ]]></code>\n                                    </Row>\n                                </Rows>\n                            </ConfigurationTable>\n                        </ConfigurationTables>\n                    </ServiceImplementation>\n                </ServiceImplementations>\n";
    write(
        &root,
        "Things/P.Caller.xml",
        &thing("P.Caller", &[], caller_members),
    );
    let solution = Solution::load(&root.join("twaco.toml")).unwrap();
    let discovered = workspace::discover(&solution);
    let extracted = workflow::extract(
        &solution,
        &discovered.entities,
        &[],
        false,
        &crate::core::lock::acquire(&solution.root, "test", &[]).unwrap(),
    );
    assert_eq!(extracted.failed, 0, "{:?}", extracted.log);
    Fixture {
        _dir: root_guard,
        root,
        solution,
    }
}

fn request(member: Member, copy: bool, from: &str, to: &str, name: &str) -> Request {
    Request {
        member,
        copy,
        from: from.into(),
        to: to.into(),
        name: name.into(),
        new_name: None,
        leave_delegate: false,
    }
}

fn out_of_step(solution: &Solution) -> Vec<String> {
    let discovered = workspace::discover(solution);
    discovered
        .entities
        .iter()
        .filter(|entity| {
            let outcome = workflow::sync(
                solution,
                std::slice::from_ref(entity),
                &[],
                workflow::SyncOptions {
                    check: true,
                    ..Default::default()
                },
            );
            outcome.changed > 0 || outcome.failed > 0
        })
        .map(|entity| entity.info.name.clone())
        .collect()
}

fn read(fixture: &Fixture, relative: &str) -> String {
    std::fs::read_to_string(fixture.root.join(relative)).unwrap()
}

#[test]
fn copying_a_service_lands_it_re_indented_with_its_script_unchanged_and_in_step() {
    let fixture = fixture();
    assert!(
        out_of_step(&fixture.solution).is_empty(),
        "the fixture starts in step"
    );
    let before = snapshot(&fixture.root);
    let planned = plan(
        &fixture.solution,
        &request(Member::Service, true, "P.Base_TS", "P.Target_TS", "Alpha"),
    )
    .unwrap();
    assert_eq!(snapshot(&fixture.root), before, "a plan writes nothing");
    assert!(planned.callers.files == 0 && planned.from_new.is_none());
    apply(&planned, &locked(&fixture)).unwrap();
    let target = read(&fixture, "ThingShapes/P.Target_TS.xml");
    // The one-line empty sections were opened, the block sits at the section's nesting.
    assert!(
        target
            .contains("<ServiceDefinitions>\n                <ServiceDefinition name=\"Alpha\">\n"),
        "{target}"
    );
    assert!(
        target.contains("</ServiceDefinition>\n            </ServiceDefinitions>"),
        "{target}"
    );
    assert!(target.contains("<ServiceImplementations>\n                <ServiceImplementation handlerName=\"Script\" name=\"Alpha\">"), "{target}");
    // The script inside CDATA is byte for byte what it was.
    assert!(target.contains(SCRIPT), "{target}");
    assert_eq!(
        snapshot(&fixture.root).get(&fixture.root.join("ThingShapes/P.Base_TS.xml")),
        before.get(&fixture.root.join("ThingShapes/P.Base_TS.xml")),
        "a copy leaves the source alone"
    );
    // A sidecar for it, and nothing out of step.
    assert!(fixture
        .root
        .join("src/P.Target_TS/services/Alpha/script.js")
        .is_file());
    assert!(
        out_of_step(&fixture.solution).is_empty(),
        "{:?}",
        out_of_step(&fixture.solution)
    );
}

#[test]
fn moving_a_service_away_lists_who_calls_it_and_removes_it_and_its_sidecar_from_the_source() {
    let fixture = fixture();
    let planned = plan(
        &fixture.solution,
        &request(Member::Service, false, "P.Base_TS", "P.Target_TS", "Alpha"),
    )
    .unwrap();
    assert!(planned.callers.references >= 1, "{:?}", planned.callers);
    assert!(
        planned
            .callers
            .first
            .iter()
            .any(|line| line.contains("P.Caller") || line.contains("Caller")),
        "{:?}",
        planned.callers
    );
    assert!(planned
        .notes
        .iter()
        .any(|note| note.contains("stop resolving")));
    apply(&planned, &locked(&fixture)).unwrap();
    let base = read(&fixture, "ThingShapes/P.Base_TS.xml");
    assert!(!base.contains("Alpha"), "{base}");
    assert!(base.contains("Level"), "the property stayed");
    assert!(
        !fixture.root.join("src/P.Base_TS/services/Alpha").exists(),
        "its sidecar went with it"
    );
    assert!(
        !fixture.root.join("src/P.Base_TS").exists(),
        "and the folders it left empty"
    );
    assert!(fixture
        .root
        .join("src/P.Target_TS/services/Alpha/script.js")
        .is_file());
    assert!(
        out_of_step(&fixture.solution).is_empty(),
        "{:?}",
        out_of_step(&fixture.solution)
    );
}

#[test]
fn moving_up_into_something_the_source_inherits_breaks_no_caller() {
    let fixture = fixture();
    // Give the instance its own service first, copied from the shape.
    apply(
        &plan(
            &fixture.solution,
            &Request {
                new_name: Some("Own".into()),
                ..request(Member::Service, true, "P.Base_TS", "P.Inst", "Alpha")
            },
        )
        .unwrap(),
        &locked(&fixture),
    )
    .unwrap();
    let planned = plan(
        &fixture.solution,
        &request(Member::Service, false, "P.Inst", "P.Target_TS", "Own"),
    )
    .unwrap();
    // Target_TS is not inherited by P.Inst, so this one does break callers (none here call Own).
    assert_eq!(planned.callers.references, 0);
    // Into the shape P.Inst implements: nothing breaks, and it says so.
    let up = plan(
        &fixture.solution,
        &request(Member::Service, false, "P.Inst", "P.Base_TS", "Own"),
    )
    .unwrap();
    assert!(
        up.notes.iter().any(|note| note.contains("keep resolving")),
        "{:?}",
        up.notes
    );
    assert_eq!(up.callers.references, 0);
}

#[test]
fn a_name_the_target_its_ancestors_or_its_descendants_already_use_is_refused() {
    let fixture = fixture();
    // The instance implements Base, which declares Alpha: Alpha cannot be added to the instance.
    let error = plan(
        &fixture.solution,
        &request(Member::Service, true, "P.Base_TS", "P.Inst", "Alpha"),
    )
    .unwrap_err();
    assert!(matches!(error, RelocateError::Exists { .. }), "{error}");
    // Nor can a second Alpha be put on the target once it has one.
    apply(
        &plan(
            &fixture.solution,
            &request(Member::Service, true, "P.Base_TS", "P.Target_TS", "Alpha"),
        )
        .unwrap(),
        &locked(&fixture),
    )
    .unwrap();
    let again = plan(
        &fixture.solution,
        &request(Member::Service, true, "P.Base_TS", "P.Target_TS", "Alpha"),
    )
    .unwrap_err();
    assert!(again.to_string().contains("already taken"), "{again}");
    // Unknown entities and members, and an inherited declaration, are named.
    assert!(matches!(
        plan(
            &fixture.solution,
            &request(Member::Service, true, "P.Nope", "P.Target_TS", "Alpha")
        ),
        Err(RelocateError::Unknown { .. })
    ));
    assert!(matches!(
        plan(
            &fixture.solution,
            &request(Member::Service, true, "P.Base_TS", "P.Target_TS", "Nope")
        ),
        Err(RelocateError::NotDeclared { .. })
    ));
    assert!(matches!(
        plan(
            &fixture.solution,
            &request(Member::Service, true, "P.Inst", "P.Target_TS", "Alpha")
        ),
        Err(RelocateError::NotDeclared { .. } | RelocateError::Inherited { .. })
    ));
    assert!(
        plan(
            &fixture.solution,
            &request(Member::Service, true, "P.Base_TS", "P.Base_TS", "Alpha")
        )
        .is_err(),
        "the same entity under the same name"
    );
}

#[test]
fn a_member_can_be_renamed_on_the_way() {
    let fixture = fixture();
    let renamed = Request {
        new_name: Some("Beta".into()),
        ..request(Member::Service, true, "P.Base_TS", "P.Target_TS", "Alpha")
    };
    let planned = plan(&fixture.solution, &renamed).unwrap();
    assert!(planned
        .notes
        .iter()
        .any(|note| note.contains("renamed to Beta")));
    apply(&planned, &locked(&fixture)).unwrap();
    let target = read(&fixture, "ThingShapes/P.Target_TS.xml");
    assert!(
        target.contains("name=\"Beta\"") && !target.contains("name=\"Alpha\""),
        "{target}"
    );
    assert!(fixture
        .root
        .join("src/P.Target_TS/services/Beta/script.js")
        .is_file());
    assert!(out_of_step(&fixture.solution).is_empty());
    // The new name is checked like any other.
    assert!(plan(
        &fixture.solution,
        &Request {
            new_name: Some("Alpha".into()),
            ..request(Member::Service, true, "P.Target_TS", "P.Base_TS", "Beta")
        }
    )
    .is_err());
}

#[test]
fn a_property_moves_into_an_empty_one_line_section() {
    let fixture = fixture();
    let planned = plan(
        &fixture.solution,
        &request(Member::Property, true, "P.Base_TS", "P.Target_TS", "Level"),
    )
    .unwrap();
    apply(&planned, &locked(&fixture)).unwrap();
    let target = read(&fixture, "ThingShapes/P.Target_TS.xml");
    assert!(target.contains("<PropertyDefinitions>\n                <PropertyDefinition baseType=\"NUMBER\" name=\"Level\"></PropertyDefinition>\n            </PropertyDefinitions>"), "{target}");
    assert!(read(&fixture, "ThingShapes/P.Base_TS.xml").contains("Level"));
    // Copying it onto something that implements the source would declare it twice; moving would not.
    let copied = plan(
        &fixture.solution,
        &request(Member::Property, true, "P.Base_TS", "P.Inst", "Level"),
    )
    .unwrap_err();
    assert!(matches!(copied, RelocateError::Exists { .. }), "{copied}");
    let moved = plan(
        &fixture.solution,
        &request(Member::Property, false, "P.Base_TS", "P.Inst", "Level"),
    )
    .unwrap();
    assert!(moved.from_new.is_some());
}

#[test]
fn a_missing_section_is_added_where_composer_would_put_it() {
    let fixture = fixture();
    write(
        &fixture.root,
        "ThingShapes/P.Bare_TS.xml",
        &shape(
            "P.Bare_TS",
            "<PropertyDefinitions></PropertyDefinitions>",
            "<ServiceDefinitions></ServiceDefinitions>",
            None,
        ),
    );
    apply(
        &plan(
            &fixture.solution,
            &request(Member::Service, true, "P.Base_TS", "P.Bare_TS", "Alpha"),
        )
        .unwrap(),
        &locked(&fixture),
    )
    .unwrap();
    let bare = read(&fixture, "ThingShapes/P.Bare_TS.xml");
    let definitions = bare.find("<ServiceDefinitions>").unwrap();
    let implementations = bare.find("<ServiceImplementations>").unwrap();
    assert!(
        definitions < implementations
            && bare.contains("</ServiceImplementations>\n        </ThingShape>"),
        "{bare}"
    );
    assert!(sidecar::extract_services(bare.as_bytes())
        .unwrap()
        .iter()
        .any(|service| service.name == "Alpha"));
}

#[test]
fn leaving_a_delegate_keeps_the_service_on_the_source_and_calls_the_moved_one() {
    let fixture = fixture();
    let delegating = Request {
        leave_delegate: true,
        ..request(Member::Service, false, "P.Base_TS", "P.Inst", "Alpha")
    };
    // The instance implements Base, which still declares Alpha: the name is taken, so use another.
    assert!(plan(&fixture.solution, &delegating).is_err());
    write(
        &fixture.root,
        "ThingShapes/P.Other_TS.xml",
        &shape(
            "P.Other_TS",
            "<PropertyDefinitions></PropertyDefinitions>",
            "<ServiceDefinitions></ServiceDefinitions>",
            Some("<ServiceImplementations></ServiceImplementations>"),
        ),
    );
    write(&fixture.root, "Things/P.Lone.xml", &thing("P.Lone", &[], "                <ServiceDefinitions></ServiceDefinitions>\n                <ServiceImplementations></ServiceImplementations>\n"));
    let delegating = Request {
        leave_delegate: true,
        ..request(Member::Service, false, "P.Base_TS", "P.Lone", "Alpha")
    };
    let planned = plan(&fixture.solution, &delegating).unwrap();
    assert!(planned.notes.iter().any(|note| note.contains("delegate")));
    apply(&planned, &locked(&fixture)).unwrap();
    let base = read(&fixture, "ThingShapes/P.Base_TS.xml");
    assert!(base.contains("name=\"Alpha\""), "the definition stays");
    assert!(
        base.contains("Things[\"P.Lone\"].Alpha({ who: who });"),
        "{base}"
    );
    assert!(
        !base.contains("hello"),
        "the old body is gone from the source"
    );
    assert!(
        read(&fixture, "Things/P.Lone.xml").contains("hello"),
        "the body moved to the target"
    );
    assert!(read(&fixture, "src/P.Base_TS/services/Alpha/script.js")
        .contains("Things[\"P.Lone\"].Alpha"));
    // A delegate needs a Thing to call and a move to leave it from.
    assert!(plan(
        &fixture.solution,
        &Request {
            leave_delegate: true,
            ..request(Member::Service, false, "P.Base_TS", "P.Other_TS", "Alpha")
        }
    )
    .is_err());
    assert!(plan(
        &fixture.solution,
        &Request {
            leave_delegate: true,
            ..request(Member::Service, true, "P.Base_TS", "P.Lone", "Alpha")
        }
    )
    .is_err());
}

#[test]
fn a_file_changed_since_the_plan_is_refused_and_a_failed_write_is_undone() {
    let fixture = fixture();
    let planned = plan(
        &fixture.solution,
        &request(Member::Service, false, "P.Base_TS", "P.Target_TS", "Alpha"),
    )
    .unwrap();
    // Saved by someone after the plan.
    let target_path = fixture.root.join("ThingShapes/P.Target_TS.xml");
    let original = std::fs::read(&target_path).unwrap();
    let mut edited = original.clone();
    edited.extend_from_slice(b"<!-- later -->\n");
    std::fs::write(&target_path, &edited).unwrap();
    // Says what came back instead: under a loaded parallel test run this has failed rarely.
    match apply(&planned, &locked(&fixture)) {
        Err(RelocateError::Refused(_)) => {}
        other => panic!("expected the changed file to be refused, got {other:?}"),
    }
    std::fs::write(&target_path, &original).unwrap();
    // A sidecar write that cannot happen (a file where the folder must go) restores the XML.
    std::fs::create_dir_all(fixture.root.join("src")).unwrap();
    std::fs::write(fixture.root.join("src/P.Target_TS"), b"in the way").unwrap();
    let before = snapshot(&fixture.root);
    assert!(apply(&planned, &locked(&fixture)).is_err());
    assert_eq!(
        snapshot(&fixture.root),
        before,
        "everything written was put back, the source sidecar included"
    );
}

/// A move written by a process that is killed at every point of the journaled write, then
/// recovered by the next command to take the lock.
#[cfg(feature = "test-failpoints")]
mod crash {
    use super::*;

    const ROOT_VARIABLE: &str = "TWACO_TEST_CHILD_ROOT";

    fn the_move(solution: &Solution) -> Plan {
        plan(
            solution,
            &request(Member::Service, false, "P.Base_TS", "P.Target_TS", "Alpha"),
        )
        .unwrap()
    }

    fn relative(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        snapshot(root)
            .into_iter()
            .filter(|(path, _)| !path.starts_with(root.join(".twaco")))
            .map(|(path, bytes)| (path.strip_prefix(root).unwrap().to_path_buf(), bytes))
            .collect()
    }

    /// Runs in the child. Does nothing when the test binary runs it as an ordinary test.
    #[test]
    fn child_moves_the_service() {
        let Ok(root) = std::env::var(ROOT_VARIABLE) else {
            return;
        };
        let solution = Solution::load(&Path::new(&root).join("twaco.toml")).unwrap();
        let planned = the_move(&solution);
        let lock = crate::core::lock::acquire(&solution.root, "child", &[]).unwrap();
        let _ = apply(&planned, &lock);
    }

    #[test]
    fn a_move_is_wholly_done_or_not_at_all_whenever_the_process_dies() {
        let before = relative(&fixture().root);
        let after = {
            let done = fixture();
            apply(&the_move(&done.solution), &locked(&done)).unwrap();
            relative(&done.root)
        };
        assert_ne!(before, after);
        let mut points: Vec<(String, bool)> = vec![
            ("after-journal".to_string(), false),
            ("after-stage".to_string(), false),
            ("after-applying".to_string(), false),
        ];
        for step in 1..=6 {
            points.push((format!("after-step-{step}-visible"), true));
            points.push((format!("after-step-{step}-marked"), true));
        }
        points.push(("after-commit".to_string(), true));
        for (point, finished) in points {
            let fixture = fixture();
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "core::relocate::tests::crash::child_moves_the_service",
                    "--nocapture",
                ])
                .env(ROOT_VARIABLE, &fixture.root)
                .env("TWACO_TEST_FAILPOINT", &point)
                .output()
                .unwrap()
                .status;
            assert!(!status.success(), "{point}: the child was not stopped");
            drop(locked(&fixture));
            let now = relative(&fixture.root);
            let hidden: Vec<&PathBuf> = now
                .keys()
                .filter(|path| {
                    let name = path.file_name().unwrap().to_string_lossy();
                    name.ends_with(".twaco-stage") || name.ends_with(".twaco-backup")
                })
                .collect();
            assert!(hidden.is_empty(), "{point}: {hidden:?}");
            let expected = if finished { &after } else { &before };
            assert_eq!(
                now.keys().collect::<Vec<_>>(),
                expected.keys().collect::<Vec<_>>(),
                "{point}"
            );
            for (path, bytes) in expected {
                assert!(&now[path] == bytes, "{point}: {} differs", path.display());
            }
        }
    }
}

#[test]
fn a_moved_services_source_folder_goes_whole_even_with_nested_content() {
    let fixture = fixture();
    let planned = plan(
        &fixture.solution,
        &request(Member::Service, false, "P.Base_TS", "P.Target_TS", "Alpha"),
    )
    .unwrap();
    let nested = fixture
        .root
        .join("src/P.Base_TS/services/Alpha/notes/deep/x.txt");
    std::fs::create_dir_all(nested.parent().unwrap()).unwrap();
    std::fs::write(&nested, b"kept in the journal's backup").unwrap();
    let applied = apply(&planned, &locked(&fixture)).unwrap();
    assert!(!fixture.root.join("src/P.Base_TS/services/Alpha").exists());
    assert_eq!(applied.removed.len(), 1);
    assert_eq!(
        snapshot(&fixture.root)
            .keys()
            .filter(|path| {
                let name = path.file_name().unwrap().to_string_lossy();
                name.ends_with(".twaco-stage") || name.ends_with(".twaco-backup")
            })
            .count(),
        0
    );
}
