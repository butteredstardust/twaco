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

const WRAP: (&str, &str) = (
    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Entities>\n",
    "</Entities>\n",
);

fn service(name: &str, script: &str) -> (String, String) {
    (
        format!("                    <ServiceDefinition name=\"{name}\">\n                        <ResultType baseType=\"NOTHING\" name=\"result\"></ResultType>\n                    </ServiceDefinition>\n"),
        format!("                    <ServiceImplementation handlerName=\"Script\" name=\"{name}\">\n                        <ConfigurationTables>\n                            <ConfigurationTable name=\"Script\">\n                                <Rows>\n                                    <Row>\n                                        <code><![CDATA[\n{script}\n                                        ]]></code>\n                                    </Row>\n                                </Rows>\n                            </ConfigurationTable>\n                        </ConfigurationTables>\n                    </ServiceImplementation>\n"),
    )
}

fn template(
    name: &str,
    base: &str,
    shapes: &[&str],
    property: &str,
    service_name: &str,
    table: Option<&str>,
) -> String {
    let (definition, implementation) = service(service_name, "return 1;");
    let implemented: String = shapes
        .iter()
        .map(|shape| {
            format!("            <ImplementedShape name=\"{shape}\"></ImplementedShape>\n")
        })
        .collect();
    let tables = table.map(|table| format!("        <ConfigurationTableDefinitions>\n            <ConfigurationTableDefinition dataShapeName=\"P.Shape\" name=\"{table}\"></ConfigurationTableDefinition>\n        </ConfigurationTableDefinitions>\n")).unwrap_or_default();
    format!(
        "{}    <ThingTemplates>\n        <ThingTemplate name=\"{name}\" projectName=\"P\" baseThingTemplate=\"{base}\">\n            <ImplementedShapes>\n{implemented}            </ImplementedShapes>\n{tables}            <ThingShape>\n                <PropertyDefinitions>\n                    <PropertyDefinition baseType=\"NUMBER\" name=\"{property}\"></PropertyDefinition>\n                </PropertyDefinitions>\n                <ServiceDefinitions>\n{definition}                </ServiceDefinitions>\n                <ServiceImplementations>\n{implementation}                </ServiceImplementations>\n            </ThingShape>\n        </ThingTemplate>\n    </ThingTemplates>\n{}",
        WRAP.0, WRAP.1
    )
}

fn thing(
    name: &str,
    template: &str,
    values: &[&str],
    tables: &[&str],
    script: Option<&str>,
) -> String {
    let properties: String = values
        .iter()
        .map(|value| {
            format!(
                "            <{value}>\n                <Value>3</Value>\n            </{value}>\n"
            )
        })
        .collect();
    let held: String = tables.iter().map(|table| format!("            <ConfigurationTable name=\"{table}\">\n                <Rows></Rows>\n            </ConfigurationTable>\n")).collect();
    let (definition, implementation) = script
        .map(|script| service("Use", script))
        .unwrap_or_default();
    format!(
        "{}    <Things>\n        <Thing name=\"{name}\" projectName=\"P\" thingTemplate=\"{template}\">\n            <ThingProperties>\n{properties}            </ThingProperties>\n            <ConfigurationTables>\n{held}            </ConfigurationTables>\n            <ThingShape>\n                <ServiceDefinitions>\n{definition}                </ServiceDefinitions>\n                <ServiceImplementations>\n{implementation}                </ServiceImplementations>\n            </ThingShape>\n        </Thing>\n    </Things>\n{}",
        WRAP.0, WRAP.1
    )
}

fn shape(name: &str) -> String {
    format!(
        "{}    <ThingShapes>\n        <ThingShape name=\"{name}\" projectName=\"P\">\n            <PropertyDefinitions></PropertyDefinitions>\n            <ServiceDefinitions></ServiceDefinitions>\n            <ServiceImplementations></ServiceImplementations>\n        </ThingShape>\n    </ThingShapes>\n{}",
        WRAP.0, WRAP.1
    )
}

fn fixture() -> Fixture {
    let root_guard = tempfile::Builder::new()
        .prefix("twaco-retemplate-")
        .tempdir()
        .unwrap();
    let root = root_guard.path().to_path_buf();
    std::fs::create_dir_all(&root).unwrap();
    write(&root, "twaco.toml", "[[project]]\nname = \"P\"\ncollections = [\"Things\", \"ThingShapes\", \"ThingTemplates\"]\n");
    write(
        &root,
        "ThingTemplates/P.Old_TT.xml",
        &template(
            "P.Old_TT",
            "GenericThing",
            &["P.A_TS"],
            "OldProp",
            "OldSvc",
            Some("OldCT"),
        ),
    );
    write(
        &root,
        "ThingTemplates/P.New_TT.xml",
        &template("P.New_TT", "GenericThing", &[], "NewProp", "NewSvc", None),
    );
    write(
        &root,
        "ThingTemplates/P.Mid_TT.xml",
        &template("P.Mid_TT", "GenericThing", &[], "MidProp", "MidSvc", None),
    );
    write(&root, "ThingShapes/P.A_TS.xml", &shape("P.A_TS"));
    write(&root, "ThingShapes/P.B_TS.xml", &shape("P.B_TS"));
    // P.T uses everything the old template gives it; P.Plain uses nothing; P.User calls P.T.
    write(
        &root,
        "Things/P.T.xml",
        &thing("P.T", "P.Old_TT", &["OldProp"], &["OldCT"], None),
    );
    write(
        &root,
        "Things/P.Plain.xml",
        &thing("P.Plain", "P.Old_TT", &[], &[], None),
    );
    write(
        &root,
        "Things/P.User.xml",
        &thing(
            "P.User",
            "GenericThing",
            &[],
            &[],
            Some("Things[\"P.T\"].OldSvc();"),
        ),
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

fn request(entity: &str) -> Request {
    Request {
        entity: entity.into(),
        ..Default::default()
    }
}

fn names(changes: &[Change]) -> Vec<String> {
    changes
        .iter()
        .map(|change| format!("{} {}", change.kind, change.name))
        .collect()
}

#[test]
fn the_plan_says_what_a_thing_gains_loses_holds_and_still_references() {
    let fixture = fixture();
    let before = snapshot(&fixture.root);
    let planned = plan(
        &fixture.solution,
        &Request {
            template: Some("P.New_TT".into()),
            ..request("P.T")
        },
    )
    .unwrap();
    assert_eq!(snapshot(&fixture.root), before, "a plan writes nothing");
    assert_eq!(planned.affected, ["P.T"]);
    assert_eq!(
        names(&planned.gained),
        ["property NewProp", "service NewSvc"]
    );
    // Lost: the old template's members, and the shape it implemented does not matter here (no members).
    assert_eq!(
        names(&planned.lost),
        [
            "configuration table OldCT",
            "property OldProp",
            "service OldSvc"
        ]
    );
    let by_name = |name: &str| {
        planned
            .lost
            .iter()
            .find(|change| change.name == name)
            .unwrap()
    };
    assert_eq!(
        by_name("OldProp").orphaned,
        1,
        "its stored value has no definition left"
    );
    assert_eq!(by_name("OldCT").orphaned, 1, "so does the table it holds");
    assert!(
        by_name("OldSvc")
            .references
            .iter()
            .any(|line| line.contains("User")),
        "{:?}",
        by_name("OldSvc").references
    );
    assert!(
        planned
            .blocked
            .iter()
            .any(|reason| reason.contains("OldProp"))
            && planned
                .blocked
                .iter()
                .any(|reason| reason.contains("OldSvc")),
        "{:?}",
        planned.blocked
    );
    // Refused without --accept-loss, and nothing is written by the refusal.
    assert!(matches!(apply(&planned), Err(RetemplateError::Loss { .. })));
    assert_eq!(snapshot(&fixture.root), before);
}

#[test]
fn with_accept_loss_only_the_template_attribute_changes() {
    let fixture = fixture();
    let before = std::fs::read_to_string(fixture.root.join("Things/P.T.xml")).unwrap();
    let planned = plan(
        &fixture.solution,
        &Request {
            template: Some("P.New_TT".into()),
            accept_loss: true,
            ..request("P.T")
        },
    )
    .unwrap();
    apply(&planned).unwrap();
    let after = std::fs::read_to_string(fixture.root.join("Things/P.T.xml")).unwrap();
    assert_eq!(
        after,
        before.replace("thingTemplate=\"P.Old_TT\"", "thingTemplate=\"P.New_TT\"")
    );
}

#[test]
fn a_loss_nothing_holds_or_references_needs_no_acceptance() {
    let fixture = fixture();
    let planned = plan(
        &fixture.solution,
        &Request {
            template: Some("P.New_TT".into()),
            ..request("P.Plain")
        },
    )
    .unwrap();
    assert_eq!(
        names(&planned.lost),
        [
            "configuration table OldCT",
            "property OldProp",
            "service OldSvc"
        ],
        "listed all the same"
    );
    assert!(planned.blocked.is_empty(), "{:?}", planned.blocked);
    apply(&planned).unwrap();
    assert!(
        std::fs::read_to_string(fixture.root.join("Things/P.Plain.xml"))
            .unwrap()
            .contains("thingTemplate=\"P.New_TT\"")
    );
}

#[test]
fn retemplating_a_template_reports_every_thing_that_inherits_it() {
    let fixture = fixture();
    let planned = plan(
        &fixture.solution,
        &Request {
            template: Some("P.Mid_TT".into()),
            ..request("P.Old_TT")
        },
    )
    .unwrap();
    // The template keeps declaring its own members; what changes is what it and its Things inherit
    // from GenericThing, which declares nothing here. So nothing is lost, and the Things are listed.
    assert!(
        planned.affected.contains(&"P.Old_TT".to_string())
            && planned.affected.contains(&"P.T".to_string())
            && planned.affected.contains(&"P.Plain".to_string()),
        "{:?}",
        planned.affected
    );
    assert_eq!(
        names(&planned.gained),
        ["property MidProp", "service MidSvc"]
    );
    assert!(planned.lost.is_empty());
    apply(&planned).unwrap();
    assert!(
        std::fs::read_to_string(fixture.root.join("ThingTemplates/P.Old_TT.xml"))
            .unwrap()
            .contains("baseThingTemplate=\"P.Mid_TT\"")
    );
    // A cycle is refused: Old_TT is now the base of nothing, but Mid_TT cannot take Old_TT as its base
    // once Old_TT has Mid_TT as its base.
    let cycle = plan(
        &fixture.solution,
        &Request {
            template: Some("P.Old_TT".into()),
            ..request("P.Mid_TT")
        },
    )
    .unwrap_err();
    assert!(cycle.to_string().contains("cycle"), "{cycle}");
}

#[test]
fn implemented_shapes_are_added_and_removed_in_place() {
    let fixture = fixture();
    let original = std::fs::read(fixture.root.join("ThingTemplates/P.Old_TT.xml")).unwrap();
    // Old_TT implements A_TS in an own-line section: add B_TS beside it, then take it away again.
    let added = plan(
        &fixture.solution,
        &Request {
            add_shapes: vec!["P.B_TS".into()],
            ..request("P.Old_TT")
        },
    )
    .unwrap();
    apply(&added).unwrap();
    let text = std::fs::read_to_string(fixture.root.join("ThingTemplates/P.Old_TT.xml")).unwrap();
    assert!(text.contains("            <ImplementedShape name=\"P.A_TS\"></ImplementedShape>\n            <ImplementedShape name=\"P.B_TS\"></ImplementedShape>\n            </ImplementedShapes>"), "{text}");
    let removed = plan(
        &fixture.solution,
        &Request {
            remove_shapes: vec!["P.B_TS".into()],
            ..request("P.Old_TT")
        },
    )
    .unwrap();
    apply(&removed).unwrap();
    assert_eq!(
        std::fs::read(fixture.root.join("ThingTemplates/P.Old_TT.xml")).unwrap(),
        original,
        "back, byte for byte"
    );
    // A Thing with no ImplementedShapes entries gets one.
    let thing_added = plan(
        &fixture.solution,
        &Request {
            add_shapes: vec!["P.A_TS".into()],
            ..request("P.Plain")
        },
    )
    .unwrap();
    apply(&thing_added).unwrap();
    let plain = std::fs::read_to_string(fixture.root.join("Things/P.Plain.xml")).unwrap();
    assert!(
        plain.contains("<ImplementedShape name=\"P.A_TS\"></ImplementedShape>"),
        "{plain}"
    );
}

#[test]
fn nonsense_requests_are_refused_before_anything_is_planned() {
    let fixture = fixture();
    assert!(plan(&fixture.solution, &request("P.T"))
        .unwrap_err()
        .to_string()
        .contains("nothing to change"));
    assert!(matches!(
        plan(
            &fixture.solution,
            &Request {
                template: Some("P.New_TT".into()),
                ..request("P.Nope")
            }
        ),
        Err(RetemplateError::Unknown { .. })
    ));
    assert!(plan(
        &fixture.solution,
        &Request {
            template: Some("P.Old_TT".into()),
            ..request("P.T")
        }
    )
    .unwrap_err()
    .to_string()
    .contains("already has template"));
    assert!(plan(
        &fixture.solution,
        &Request {
            remove_shapes: vec!["P.B_TS".into()],
            ..request("P.T")
        }
    )
    .unwrap_err()
    .to_string()
    .contains("does not itself implement"));
    assert!(plan(
        &fixture.solution,
        &Request {
            add_shapes: vec!["P.A_TS".into()],
            ..request("P.Old_TT")
        }
    )
    .unwrap_err()
    .to_string()
    .contains("already implements"));
    // A shape is not a thing to retemplate.
    assert!(matches!(
        plan(
            &fixture.solution,
            &Request {
                template: Some("P.New_TT".into()),
                ..request("P.A_TS")
            }
        ),
        Err(RetemplateError::Unknown { .. })
    ));
}

#[test]
fn a_file_changed_since_the_plan_is_refused() {
    let fixture = fixture();
    let planned = plan(
        &fixture.solution,
        &Request {
            template: Some("P.New_TT".into()),
            accept_loss: true,
            ..request("P.Plain")
        },
    )
    .unwrap();
    let path = fixture.root.join("Things/P.Plain.xml");
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.extend_from_slice(b"<!-- edited -->\n");
    std::fs::write(&path, bytes).unwrap();
    assert!(apply(&planned).is_err());
}

#[test]
fn a_name_is_written_escaped_and_reads_back_as_given() {
    let fixture = fixture();
    let found = workspace::discover(&fixture.solution).entities;
    let file = found.iter().find(|e| e.info.name == "P.T").unwrap();
    let src = std::fs::read(&file.path).unwrap();
    let request = Request {
        template: Some("P.B&C".to_string()),
        add_shapes: vec!["P.Odd\" x=\"1".to_string()],
        ..request("P.T")
    };
    let out = edit_document(&src, file, &request).unwrap();
    let text = String::from_utf8(out.clone()).unwrap();
    assert!(text.contains("thingTemplate=\"P.B&amp;C\""), "{text}");
    assert!(!text.contains(" x=\"1\""), "no attribute was added: {text}");
    // What a reader of the document sees is exactly the name asked for.
    let tokens = scan::tokenize(&out).unwrap();
    let names: Vec<String> = tokens
        .iter()
        .filter(|t| t.name.of(&out) == b"ImplementedShape")
        .filter_map(|t| scan::attribute(&out, t, "name").ok().flatten())
        .map(|span| scan::decode_entities(&String::from_utf8_lossy(span.of(&out))))
        .collect();
    assert!(names.contains(&"P.Odd\" x=\"1".to_string()), "{names:?}");
}

#[test]
fn an_empty_name_or_a_control_character_is_refused() {
    let fixture = fixture();
    for bad in ["", "P.\u{7}Bell"] {
        for wanted in [
            Request {
                template: Some(bad.to_string()),
                ..request("P.T")
            },
            Request {
                add_shapes: vec![bad.to_string()],
                ..request("P.T")
            },
        ] {
            match plan(&fixture.solution, &wanted) {
                Err(RetemplateError::Invalid(why)) => {
                    assert!(why.contains("is not an entity name"), "{bad:?}: {why}")
                }
                other => panic!("{bad:?}: {other:?}"),
            }
        }
    }
}
