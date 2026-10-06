use super::super::config::Solution;
use super::super::entity_key::ServiceTarget;
use super::super::server::Method;
use super::super::server::ServerError;
use super::compiler::{
    compiler_command, map_finding, parse_compiler_output, write_check_project, CheckProject,
    CompilerFinding,
};
use super::model::{parse_document, DataShape, Model, Parsed, TypedValue};
use super::platform::{trim_metadata, Platform, PlatformMeta, PlatformProperty};
use super::render::{generate, identifiers, type_name, Generated};
use super::write::{gitignore_covers_types, render_globals, render_jsconfig};
use super::*;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::Duration;

fn document(collection: &str, body: &str) -> String {
    format!("<Entities><{collection}>{body}</{collection}></Entities>")
}

fn entity(collection: &str, body: &str) -> Entity {
    let name_start = body.find("name=\"").unwrap() + 6;
    let name_end = name_start + body[name_start..].find('"').unwrap();
    match parse_document(
        document(collection, body).as_bytes(),
        collection,
        &body[name_start..name_end],
    )
    .unwrap()
    {
        Parsed::Entity(entity) => entity,
        Parsed::DataShape(_) => panic!("expected entity"),
    }
}

fn shape(body: &str) -> DataShape {
    match parse_document(
        document("DataShapes", body).as_bytes(),
        "DataShapes",
        "Rows",
    )
    .unwrap()
    {
        Parsed::DataShape(shape) => shape,
        Parsed::Entity(_) => panic!("expected DataShape"),
    }
}

fn rendered(model: Model) -> Generated {
    generate(&model, None)
}

fn platform_property(base_type: &str) -> PlatformProperty {
    PlatformProperty {
        base_type: base_type.to_string(),
        data_shape: None,
        description: String::new(),
    }
}

fn temporary_root(label: &str) -> std::path::PathBuf {
    let nonce = crate::test_nonce();
    std::env::temp_dir().join(format!(
        "twaco-types-{label}-{}-{nonce}",
        std::process::id()
    ))
}

fn fixture_solution(
    label: &str,
    entities: &[(&str, &str, &str)],
) -> (std::path::PathBuf, Solution) {
    let root = temporary_root(label);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\ncollections = [\"Things\", \"ThingTemplates\", \"ThingShapes\"]\n").unwrap();
    for (collection, name, xml) in entities {
        std::fs::create_dir_all(root.join(collection)).unwrap();
        std::fs::write(root.join(collection).join(format!("{name}.xml")), xml).unwrap();
    }
    let solution = Solution::discover(&root).unwrap();
    (root, solution)
}

type FakeReplies = BTreeMap<(String, String), Result<Option<Value>, ServerError>>;

struct FakeRemote {
    calls: RefCell<Vec<(String, String, Value)>>,
    replies: RefCell<FakeReplies>,
}

impl FakeRemote {
    fn new(replies: FakeReplies) -> Self {
        Self {
            calls: RefCell::new(Vec::new()),
            replies: RefCell::new(replies),
        }
    }
}

impl Remote for FakeRemote {
    fn call(
        &self,
        target: &ServiceTarget,
        service: &str,
        parameters: &Value,
    ) -> Result<Option<Value>, ServerError> {
        self.calls
            .borrow_mut()
            .push((target.to_string(), service.to_string(), parameters.clone()));
        self.replies
            .borrow_mut()
            .remove(&(target.to_string(), service.to_string()))
            .unwrap_or_else(|| panic!("unexpected call {target}.{service}"))
    }
}

fn metadata(property: &str) -> Value {
    json!({
        "serviceDefinitions": {
            "Run": {
                "description": "",
                "Inputs": { "fieldDefinitions": {
                    "optional": { "baseType": "STRING", "description": "", "aspects": {
                        "isRequired": false, "dataShape": ""
                    }},
                    "needed": { "baseType": "INFOTABLE", "description": "Rows", "aspects": {
                        "isRequired": true, "dataShape": "ExternalRows"
                    }}
                }},
                "Outputs": { "baseType": "NOTHING", "dataShape": "" }
            }
        },
        "propertyDefinitions": {
            property: { "baseType": "STRING", "description": "", "aspects": { "dataShape": "" } }
        }
    })
}

fn ok(value: Value) -> Result<Option<Value>, ServerError> {
    Ok(Some(value))
}

fn service_of(entity: &Entity) -> &Service {
    entity
        .members
        .iter()
        .find_map(|member| match member {
            Member::Service(service) => Some(service),
            Member::Property(_) => None,
        })
        .unwrap()
}

#[test]
fn jsconfig_points_to_types_from_flat_and_nested_src_roots() {
    let flat = render_jsconfig(
        Path::new("solution"),
        Path::new("solution/src/T/services/S"),
    );
    assert_eq!(
        flat,
        concat!(
            "{\n",
            "  \"compilerOptions\": {\n",
            "    \"allowJs\": true,\n",
            "    \"checkJs\": false,\n",
            "    \"noEmit\": true,\n",
            "    \"target\": \"ES2015\",\n",
            "    \"lib\": [\n",
            "      \"ES2015\"\n",
            "    ],\n",
            "    \"types\": []\n",
            "  },\n",
            "  \"include\": [\n",
            "    \"script.js\",\n",
            "    \"twaco-globals.d.ts\",\n",
            "    \"../../../../.twaco/types/*.d.ts\"\n",
            "  ]\n",
            "}\n"
        )
    );

    let nested = render_jsconfig(
        Path::new("solution"),
        Path::new("solution/Project Files/X-SourceControl/src/T/services/S"),
    );
    assert!(nested.contains("../../../../../../.twaco/types/*.d.ts"));
}

#[test]
fn globals_use_the_owning_thing_template_and_shape_interfaces() {
    for (collection, body, expected, result) in [
        (
            "Things",
            r#"<Thing name="A.Thing"><ThingShape><ServiceDefinitions><ServiceDefinition name="Run"><ParameterDefinitions><FieldDefinition name="count" baseType="NUMBER" description="How many"/></ParameterDefinitions><ResultType baseType="STRING"/></ServiceDefinition></ServiceDefinitions></ThingShape></Thing>"#,
            "E_A_Thing",
            "declare let result: string;",
        ),
        (
            "ThingTemplates",
            r#"<ThingTemplate name="A.Template"><ThingShape><ServiceDefinitions><ServiceDefinition name="Run"><ResultType baseType="BOOLEAN"/></ServiceDefinition></ServiceDefinitions></ThingShape></ThingTemplate>"#,
            "E_A_Template",
            "declare let result: boolean;",
        ),
        (
            "ThingShapes",
            r#"<ThingShape name="A.Shape"><ServiceDefinitions><ServiceDefinition name="Run"><ResultType baseType="DATETIME"/></ServiceDefinition></ServiceDefinitions></ThingShape>"#,
            "E_A_Shape",
            "declare let result: Date;",
        ),
    ] {
        let owner = entity(collection, body);
        let globals = render_globals(
            service_of(&owner),
            expected,
            "result = 1;",
            &BTreeSet::new(),
            &BTreeMap::new(),
        );
        assert!(globals.starts_with(&format!("declare const me: twx.{expected};\n")));
        assert!(globals.contains(result));
        if collection == "Things" {
            assert!(globals.contains("/** How many */\ndeclare let count: number;"));
        }
    }
}

#[test]
fn gitignore_must_cover_shared_and_per_service_generated_files() {
    let nonce = crate::test_nonce();
    let root =
        std::env::temp_dir().join(format!("twaco-types-ignore-{}-{nonce}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gitignore"), ".twaco/types/\n").unwrap();
    assert!(!gitignore_covers_types(&root));
    std::fs::write(
        root.join(".gitignore"),
        ".twaco/types/\n**/services/*/jsconfig.json\n**/services/*/twaco-globals.d.ts\n",
    )
    .unwrap();
    assert!(gitignore_covers_types(&root));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn top_level_declarations_skip_the_matching_globals_only() {
    let owner = entity(
        "Things",
        r#"<Thing name="T"><ThingShape><ServiceDefinitions><ServiceDefinition name="Run"><ParameterDefinitions><FieldDefinition name="p" baseType="STRING"/></ParameterDefinitions><ResultType baseType="NUMBER"/></ServiceDefinition></ServiceDefinitions></ThingShape></Thing>"#,
    );
    let service = service_of(&owner);
    for declaration in ["let result = 1;", "const result = 1;", "var result = 1;"] {
        let globals = render_globals(
            service,
            "E_T",
            declaration,
            &BTreeSet::new(),
            &BTreeMap::new(),
        );
        assert!(globals.contains("// Skipped result:"), "{declaration}");
        assert!(!globals.contains("declare let result:"), "{declaration}");
    }
    let parameter = render_globals(
        service,
        "E_T",
        "function p() {}",
        &BTreeSet::new(),
        &BTreeMap::new(),
    );
    assert!(parameter.contains("// Skipped p:"));
    assert!(!parameter.contains("declare let p:"));
    assert!(parameter.contains("declare let result: number;"));

    let indented = render_globals(
        service,
        "E_T",
        "function f() {\n    let result = 1;\n}",
        &BTreeSet::new(),
        &BTreeMap::new(),
    );
    assert!(indented.contains("declare let result: number;"));
    assert!(!indented.contains("// Skipped result:"));
}

#[test]
fn void_service_has_no_result_global() {
    let owner = entity(
        "ThingShapes",
        r#"<ThingShape name="S"><ServiceDefinitions><ServiceDefinition name="Run"><ResultType baseType="NOTHING"/></ServiceDefinition></ServiceDefinitions></ThingShape>"#,
    );
    let globals = render_globals(
        service_of(&owner),
        "E_S",
        "result = 1;",
        &BTreeSet::new(),
        &BTreeMap::new(),
    );
    assert_eq!(globals, "declare const me: twx.E_S;\n");
}

#[test]
fn service_project_generation_is_byte_deterministic() {
    let owner = entity(
        "Things",
        r#"<Thing name="T"><ThingShape><ServiceDefinitions><ServiceDefinition name="Run"><ParameterDefinitions><FieldDefinition name="p" baseType="BOOLEAN" description="Flag"/></ParameterDefinitions><ResultType baseType="NUMBER"/></ServiceDefinition></ServiceDefinitions></ThingShape></Thing>"#,
    );
    let first = render_globals(
        service_of(&owner),
        "E_T",
        "result = p;",
        &BTreeSet::new(),
        &BTreeMap::new(),
    );
    let second = render_globals(
        service_of(&owner),
        "E_T",
        "result = p;",
        &BTreeSet::new(),
        &BTreeMap::new(),
    );
    assert_eq!(first.as_bytes(), second.as_bytes());
    assert_eq!(
        render_jsconfig(Path::new("r"), Path::new("r/src/T/services/Run")),
        render_jsconfig(Path::new("r"), Path::new("r/src/T/services/Run"))
    );
}

#[test]
fn maps_thingworx_base_types() {
    let known = BTreeSet::new();
    let ids = BTreeMap::new();
    for (base, expected) in [
        ("NUMBER", "number"),
        ("INTEGER", "number"),
        ("LONG", "number"),
        ("BOOLEAN", "boolean"),
        ("DATETIME", "Date"),
        ("JSON", "any"),
        ("NOTHING", "void"),
        ("LOCATION", "twx.LOCATION"),
        ("STRING", "string"),
        ("THINGNAME", "string"),
        ("QUERY", "any"),
        ("BOGUS", "any"),
    ] {
        assert_eq!(
            type_name(
                &TypedValue {
                    base_type: base.into(),
                    data_shape: None
                },
                &known,
                &ids
            ),
            expected
        );
    }
}

#[test]
fn infotable_uses_only_an_in_solution_datashape() {
    let model = Model {
        data_shapes: vec![shape(
            r#"<DataShape name="Rows"><FieldDefinitions/></DataShape>"#,
        )],
        entities: vec![entity(
            "Things",
            r#"<Thing name="T"><ThingShape><PropertyDefinitions>
                <PropertyDefinition name="Known" baseType="INFOTABLE" aspect.dataShape="Rows"/>
                <PropertyDefinition name="Unknown" baseType="INFOTABLE" aspect.dataShape="Elsewhere"/>
            </PropertyDefinitions></ThingShape></Thing>"#,
        )],
    };
    let text = rendered(model).entities;
    assert!(text.contains("Known: twx.INFOTABLE<twx.ds.D_Rows>;"));
    assert!(text.contains("Unknown: twx.INFOTABLE<any>;"));
}

#[test]
fn flattens_template_chain_and_shapes_with_nearest_service_winning() {
    let own = entity(
        "Things",
        r#"<Thing name="T" thingTemplate="Near"><ThingShape><ServiceDefinitions>
            <ServiceDefinition name="Repeat"><ResultType baseType="STRING"/></ServiceDefinition>
        </ServiceDefinitions></ThingShape></Thing>"#,
    );
    let near = entity(
        "ThingTemplates",
        r#"<ThingTemplate name="Near" baseThingTemplate="Base"><ThingShape>
            <PropertyDefinitions><PropertyDefinition name="NearProp" baseType="NUMBER"/></PropertyDefinitions>
            <ServiceDefinitions><ServiceDefinition name="Repeat"><ResultType baseType="NUMBER"/></ServiceDefinition></ServiceDefinitions>
        </ThingShape><ImplementedShapes><ImplementedShape name="S"/></ImplementedShapes></ThingTemplate>"#,
    );
    let base = entity(
        "ThingTemplates",
        r#"<ThingTemplate name="Base"><ThingShape>
            <PropertyDefinitions><PropertyDefinition name="BaseProp" baseType="BOOLEAN"/></PropertyDefinitions>
        </ThingShape></ThingTemplate>"#,
    );
    let implemented = entity(
        "ThingShapes",
        r#"<ThingShape name="S"><PropertyDefinitions>
            <PropertyDefinition name="ShapeProp" baseType="DATETIME"/>
        </PropertyDefinitions></ThingShape>"#,
    );
    let text = rendered(Model {
        entities: vec![base, near, implemented, own],
        data_shapes: vec![],
    })
    .entities;
    let thing = text
        .split("interface E_T")
        .nth(1)
        .unwrap()
        .split("    }")
        .next()
        .unwrap();
    assert!(thing.contains("NearProp: number"));
    assert!(thing.contains("BaseProp: boolean"));
    assert!(thing.contains("ShapeProp: Date"));
    assert!(thing.contains("Repeat(): string"));
    assert!(!thing.contains("Repeat(): number"));
}

#[test]
fn opens_external_template_chains_but_not_closed_solution_chains() {
    let open = entity(
        "Things",
        r#"<Thing name="Open" thingTemplate="GenericThing"><ThingShape/></Thing>"#,
    );
    let closed = entity(
        "Things",
        r#"<Thing name="Closed" thingTemplate="Local"><ThingShape/></Thing>"#,
    );
    let local = entity(
        "ThingTemplates",
        r#"<ThingTemplate name="Local"><ThingShape/></ThingTemplate>"#,
    );
    let shape = entity("ThingShapes", r#"<ThingShape name="AlwaysOpen"/>"#);
    let text = rendered(Model {
        entities: vec![closed, local, open, shape],
        data_shapes: vec![],
    })
    .entities;
    let open_body = text
        .split("interface E_Open")
        .nth(1)
        .unwrap()
        .split("    }")
        .next()
        .unwrap();
    let closed_body = text
        .split("interface E_Closed")
        .nth(1)
        .unwrap()
        .split("    }")
        .next()
        .unwrap();
    let shape_body = text
        .split("interface E_AlwaysOpen")
        .nth(1)
        .unwrap()
        .split("    }")
        .next()
        .unwrap();
    assert!(open_body.contains("[member: string]: any"));
    assert!(!closed_body.contains("[member: string]: any"));
    assert!(shape_body.contains("[member: string]: any"));
}

#[test]
fn suffixes_identifier_collisions_in_sorted_name_order() {
    let ids = identifiers(["A-B", "A.B", "A_B"], "E_");
    assert_eq!(ids["A-B"], "E_A_B");
    assert_eq!(ids["A.B"], "E_A_B_2");
    assert_eq!(ids["A_B"], "E_A_B_3");

    // A suffix never lands on another name's own identifier.
    let ids = identifiers(["A-B", "A.B", "A_B_2"], "E_");
    let unique: BTreeSet<&String> = ids.values().collect();
    assert_eq!(unique.len(), 3, "{ids:?}");
}

#[test]
fn required_parameter_requires_the_argument_and_member() {
    let service = entity(
        "Things",
        r#"<Thing name="T"><ThingShape><ServiceDefinitions>
            <ServiceDefinition name="Run"><ParameterDefinitions>
                <FieldDefinition name="needed" baseType="STRING" aspect.isRequired="true"/>
                <FieldDefinition name="maybe" baseType="NUMBER"/>
            </ParameterDefinitions><ResultType baseType="NOTHING"/></ServiceDefinition>
        </ServiceDefinitions></ThingShape></Thing>"#,
    );
    let text = rendered(Model {
        entities: vec![service],
        data_shapes: vec![],
    })
    .entities;
    assert!(text.contains("Run(params: { maybe?: number; needed: string }): void;"));
}

#[test]
fn escapes_a_jsdoc_terminator() {
    let property = entity(
        "Things",
        r#"<Thing name="T"><ThingShape><PropertyDefinitions>
            <PropertyDefinition name="P" baseType="STRING" description="before */ after"/>
        </PropertyDefinitions></ThingShape></Thing>"#,
    );
    let text = rendered(Model {
        entities: vec![property],
        data_shapes: vec![],
    })
    .entities;
    assert!(text.contains("before *\\/ after"));
    assert!(!text.contains("before */ after"));
}

#[test]
fn generation_is_byte_deterministic() {
    let model = Model {
        entities: vec![entity("Things", r#"<Thing name="B"><ThingShape/></Thing>"#)],
        data_shapes: vec![shape(
            r#"<DataShape name="Rows"><FieldDefinitions>
                <FieldDefinition name="z" baseType="STRING"/><FieldDefinition name="a" baseType="NUMBER"/>
            </FieldDefinitions></DataShape>"#,
        )],
    };
    let first = generate(&model, None);
    let second = generate(&model, None);
    assert_eq!(first.datashapes.as_bytes(), second.datashapes.as_bytes());
    assert_eq!(first.entities.as_bytes(), second.entities.as_bytes());
    assert_eq!(first.collections.as_bytes(), second.collections.as_bytes());
}

#[test]
fn platform_members_close_external_roots_and_keep_solution_precedence() {
    let model = Model {
        entities: vec![entity(
            "Things",
            r#"<Thing name="T" thingTemplate="External"><ThingShape><PropertyDefinitions>
                <PropertyDefinition name="Same" baseType="NUMBER"/>
                </PropertyDefinitions></ThingShape><ImplementedShapes><ImplementedShape name="ExternalShape"/></ImplementedShapes></Thing>"#,
        )],
        data_shapes: vec![],
    };
    let platform = Platform {
        templates: BTreeMap::from([(
            "External".into(),
            PlatformMeta {
                properties: BTreeMap::from([
                    ("Same".into(), platform_property("BOOLEAN")),
                    ("TemplateOnly".into(), platform_property("STRING")),
                ]),
                ..PlatformMeta::default()
            },
        )]),
        shapes: BTreeMap::from([(
            "ExternalShape".into(),
            PlatformMeta {
                properties: BTreeMap::from([
                    ("TemplateOnly".into(), platform_property("NUMBER")),
                    ("ShapeOnly".into(), platform_property("DATETIME")),
                ]),
                ..PlatformMeta::default()
            },
        )]),
        ..Platform::default()
    };
    let text = generate(&model, Some(&platform)).entities;
    let body = text
        .split("interface E_T")
        .nth(1)
        .unwrap()
        .split("    }")
        .next()
        .unwrap();
    assert!(body.contains("Same: number"));
    assert!(!body.contains("Same: boolean"));
    assert!(
        body.contains("TemplateOnly: number"),
        "shape precedes template: {body}"
    );
    assert!(body.contains("ShapeOnly: Date"));
    assert!(!body.contains("[member: string]: any"));
}

#[test]
fn an_uncached_root_stays_open_and_a_shape_me_gets_generic_members() {
    let model = Model {
        entities: vec![
            entity(
                "Things",
                r#"<Thing name="T" thingTemplate="Missing"><ThingShape/></Thing>"#,
            ),
            entity(
                "ThingShapes",
                r#"<ThingShape name="S"><PropertyDefinitions><PropertyDefinition name="Own" baseType="NUMBER"/></PropertyDefinitions></ThingShape>"#,
            ),
        ],
        data_shapes: vec![],
    };
    let platform = Platform {
        templates: BTreeMap::from([(
            "GenericThing".into(),
            PlatformMeta {
                properties: BTreeMap::from([("name".into(), platform_property("STRING"))]),
                ..PlatformMeta::default()
            },
        )]),
        ..Platform::default()
    };
    let text = generate(&model, Some(&platform)).entities;
    let thing = text
        .split("interface E_T")
        .nth(1)
        .unwrap()
        .split("    }")
        .next()
        .unwrap();
    let shape = text
        .split("interface E_S")
        .nth(1)
        .unwrap()
        .split("    }")
        .next()
        .unwrap();
    assert!(thing.contains("[member: string]: any"));
    assert!(shape.contains("Own: number"));
    assert!(shape.contains("name: string"));
    assert!(shape.contains("[member: string]: any"));
}

#[test]
fn cached_resources_get_their_own_interfaces_and_unknown_fallback() {
    let platform = Platform {
        resources: BTreeMap::from([(
            "InfoTableFunctions".into(),
            PlatformMeta {
                properties: BTreeMap::from([("Version".into(), platform_property("STRING"))]),
                ..PlatformMeta::default()
            },
        )]),
        ..Platform::default()
    };
    let text = generate(&Model::default(), Some(&platform)).collections;
    assert!(text.contains("\"InfoTableFunctions\": twx.R_InfoTableFunctions;"));
    assert!(text.contains("interface R_InfoTableFunctions"));
    assert!(text.contains("Version: string;"));
    assert!(text.contains("declare const Resources: twx.ResourcesMap & { [name: string]: any };"));
    let without = generate(&Model::default(), None).collections;
    assert!(without.contains("declare const Resources: { [name: string]: any };"));
    assert!(!without.contains("ResourcesMap"));
}

#[test]
fn trimming_omits_empty_optional_fields_and_is_deterministic() {
    let trimmed = trim_metadata(&metadata("P")).unwrap();
    let first = serde_json::to_string_pretty(&trimmed).unwrap();
    let second = serde_json::to_string_pretty(&trimmed).unwrap();
    assert_eq!(first, second);
    assert!(!first.contains("\"description\": \"\""));
    assert!(!first.contains("\"required\": false"));
    assert!(!first.contains("\"dataShape\": \"\""));
    assert!(first.contains("\"required\": true"));
    assert!(first.contains("\"dataShape\": \"ExternalRows\""));
}

#[test]
fn fetches_solution_platform_dependencies_and_all_resources() {
    let (root, solution) = fixture_solution(
        "fetch",
        &[
            (
                "Things",
                "T",
                r#"<Entities><Things><Thing name="T" thingTemplate="Local" projectName="P"><ThingShape/><ImplementedShapes><ImplementedShape name="OutsideShape"/></ImplementedShapes></Thing></Things></Entities>"#,
            ),
            (
                "ThingTemplates",
                "Local",
                r#"<Entities><ThingTemplates><ThingTemplate name="Local" baseThingTemplate="OutsideTemplate" projectName="P"><ThingShape/></ThingTemplate></ThingTemplates></Entities>"#,
            ),
        ],
    );
    let mut replies = BTreeMap::new();
    for (collection, name) in [
        ("ThingTemplates", "GenericThing"),
        ("ThingTemplates", "OutsideTemplate"),
        ("ThingShapes", "OutsideShape"),
    ] {
        replies.insert(
            (
                format!("{collection}/{name}"),
                "GetInstanceMetadataAsJSON".into(),
            ),
            ok(metadata(name)),
        );
    }
    replies.insert(
        ("Resources/EntityServices".into(), "GetEntityList".into()),
        ok(json!({ "rows": [{"name": "Zed"}, {"name": "Alpha"}] })),
    );
    for name in ["Alpha", "Zed"] {
        replies.insert(
            (format!("Resources/{name}"), "GetMetadataAsJSON".into()),
            ok(metadata(name)),
        );
    }
    let remote = FakeRemote::new(replies);
    let outcome = fetch_platform(&remote, &solution).unwrap();
    assert_eq!(
        (outcome.templates, outcome.shapes, outcome.resources),
        (2, 1, 2)
    );
    let calls = remote.calls.borrow();
    assert!(calls
        .iter()
        .any(|(target, _, _)| target == "ThingTemplates/OutsideTemplate"));
    assert!(!calls
        .iter()
        .any(|(target, _, _)| target == "ThingTemplates/Local"));
    let list = calls
        .iter()
        .find(|(target, _, _)| target == "Resources/EntityServices")
        .unwrap();
    assert_eq!(list.2, json!({ "type": "Resource", "maxItems": 1000 }));
    let cache = std::fs::read_to_string(root.join(".twaco/platform.json")).unwrap();
    assert!(cache.ends_with('\n'));
    assert!(cache.find("\"Alpha\"").unwrap() < cache.find("\"Zed\"").unwrap());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_not_found_entity_is_skipped_but_an_auth_failure_writes_nothing() {
    let (root, solution) = fixture_solution(
        "failures",
        &[(
            "Things",
            "T",
            r#"<Entities><Things><Thing name="T" thingTemplate="Absent" projectName="P"><ThingShape/></Thing></Things></Entities>"#,
        )],
    );
    let not_found = ServerError::Http {
        method: Method::Post,
        status: 404,
        url: "test/Absent".into(),
        body: String::new(),
    };
    let remote = FakeRemote::new(BTreeMap::from([
        (
            (
                "ThingTemplates/Absent".into(),
                "GetInstanceMetadataAsJSON".into(),
            ),
            Err(not_found),
        ),
        (
            (
                "ThingTemplates/GenericThing".into(),
                "GetInstanceMetadataAsJSON".into(),
            ),
            ok(metadata("name")),
        ),
        (
            ("Resources/EntityServices".into(), "GetEntityList".into()),
            ok(json!({"rows": []})),
        ),
    ]));
    let outcome = fetch_platform(&remote, &solution).unwrap();
    assert_eq!(outcome.templates, 1);
    assert_eq!(outcome.skipped.len(), 1);
    assert!(root.join(".twaco/platform.json").is_file());

    let old_cache = b"old cache that must survive";
    std::fs::write(root.join(".twaco/platform.json"), old_cache).unwrap();
    let unauthorized = ServerError::Http {
        method: Method::Post,
        status: 401,
        url: "test/GenericThing".into(),
        body: "unauthorized".into(),
    };
    let remote = FakeRemote::new(BTreeMap::from([
        (
            (
                "ThingTemplates/Absent".into(),
                "GetInstanceMetadataAsJSON".into(),
            ),
            Err(ServerError::Http {
                method: Method::Post,
                status: 404,
                url: "test/Absent".into(),
                body: String::new(),
            }),
        ),
        (
            (
                "ThingTemplates/GenericThing".into(),
                "GetInstanceMetadataAsJSON".into(),
            ),
            Err(unauthorized),
        ),
    ]));
    assert!(fetch_platform(&remote, &solution).is_err());
    assert_eq!(
        std::fs::read(root.join(".twaco/platform.json")).unwrap(),
        old_cache
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn malformed_cache_is_reported_and_ignored() {
    let (root, solution) = fixture_solution(
        "malformed",
        &[(
            "Things",
            "T",
            r#"<Entities><Things><Thing name="T" thingTemplate="GenericThing" projectName="P"><ThingShape/></Thing></Things></Entities>"#,
        )],
    );
    std::fs::create_dir_all(root.join(".twaco")).unwrap();
    std::fs::write(root.join(".twaco/platform.json"), "{not json").unwrap();
    let outcome = write(&solution).unwrap();
    assert!(outcome
        .skipped
        .iter()
        .any(|message| message.contains("malformed and was ignored")));
    let entities = std::fs::read_to_string(root.join(".twaco/types/entities.d.ts")).unwrap();
    assert!(entities.contains("[member: string]: any"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn writing_the_same_generated_bytes_twice_is_a_fixed_point() {
    let nonce = crate::test_nonce();
    let directory =
        std::env::temp_dir().join(format!("twaco-types-{}-{nonce}", std::process::id()));
    let path = directory.join("types.d.ts");
    assert!(workspace::write_lf_if_changed(&path, "one\r\ntwo\r\n").unwrap());
    let first = std::fs::read(&path).unwrap();
    assert!(!workspace::write_lf_if_changed(&path, "one\ntwo\n").unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), first);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn generated_service_projects_do_not_change_check_gate_results() {
    let nonce = crate::test_nonce();
    let root =
        std::env::temp_dir().join(format!("twaco-types-check-{}-{nonce}", std::process::id()));
    let service_dir = root.join("src/T/services/Run");
    std::fs::create_dir_all(root.join("Things")).unwrap();
    std::fs::create_dir_all(&service_dir).unwrap();
    std::fs::write(
        root.join("twaco.toml"),
        "[[project]]\nname = \"P\"\ncollections = [\"Things\"]\n",
    )
    .unwrap();
    std::fs::write(
        root.join("Things/T.xml"),
        concat!(
            "<Entities><Things><Thing name=\"T\" projectName=\"P\"><ThingShape>",
            "<ServiceDefinitions><ServiceDefinition name=\"Run\"><ResultType baseType=\"NUMBER\"/>",
            "</ServiceDefinition></ServiceDefinitions>",
            "<ServiceImplementations><ServiceImplementation name=\"Run\" handlerName=\"Script\">",
            "<ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row>",
            "<code><![CDATA[result = 1;]]></code>",
            "</Row></Rows></ConfigurationTable></ConfigurationTables>",
            "</ServiceImplementation></ServiceImplementations>",
            "</ThingShape></Thing></Things></Entities>"
        ),
    )
    .unwrap();
    std::fs::write(
        service_dir.join("definition.xml"),
        "<ServiceDefinition name=\"Run\"><ResultType baseType=\"NUMBER\"/></ServiceDefinition>\n",
    )
    .unwrap();
    std::fs::write(service_dir.join("script.js"), "result = 1;\n").unwrap();

    let solution = Solution::load(&root.join("twaco.toml")).unwrap();
    let before = crate::core::check::run(&solution);
    write(&solution).unwrap();
    let after = crate::core::check::run(&solution);
    assert_eq!(before.gates.len(), after.gates.len());
    for (before, after) in before.gates.iter().zip(&after.gates) {
        assert_eq!(before.name, after.name);
        assert_eq!(before.examined, after.examined, "{}", before.name);
        assert_eq!(before.findings, after.findings, "{}", before.name);
        assert_eq!(before.prose, after.prose, "{}", before.name);
        assert_eq!(before.broken, after.broken, "{}", before.name);
        assert_eq!(before.gates_the_run, after.gates_the_run, "{}", before.name);
    }
    let _ = std::fs::remove_dir_all(root);
}

fn check_fixture(label: &str, script: &str) -> (PathBuf, Solution) {
    let xml = concat!(
        "<Entities><Things><Thing name=\"T\" projectName=\"P\"><ThingShape>",
        "<ServiceDefinitions><ServiceDefinition name=\"Run\"><ParameterDefinitions>",
        "<FieldDefinition name=\"input\" baseType=\"STRING\"/>",
        "</ParameterDefinitions><ResultType baseType=\"NUMBER\"/></ServiceDefinition>",
        "</ServiceDefinitions></ThingShape></Thing></Things></Entities>"
    );
    let (root, solution) = fixture_solution(label, &[("Things", "T", xml)]);
    let directory = root.join("src/T/services/Run");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("script.js"), script.as_bytes()).unwrap();
    (root, solution)
}

#[test]
fn check_file_has_typed_header_and_verbatim_script_but_no_skipped_global() {
    let script = "let input = 'local';\r\nresult = input.length;\r\n";
    let (root, solution) = check_fixture("check-project", script);
    let (model, _) = load_model(&solution);
    let projects = write_check_project(&solution, &model).unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].generated_name, "s0000.js");
    assert_eq!(projects[0].header_lines, 3);
    let generated = std::fs::read(root.join(".twaco/types/check/s0000.js")).unwrap();
    let header = concat!(
        "export {};\n",
        "/** @type {twx.E_T} */ var me;\n",
        "/** @type {number} */ var result;\n"
    );
    assert!(generated.starts_with(header.as_bytes()));
    assert_eq!(&generated[header.len()..], script.as_bytes());
    assert!(!String::from_utf8_lossy(&generated).contains("var input;"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn parses_compiler_findings_continuations_and_windows_paths_with_spaces() {
    let parsed = parse_compiler_output(concat!(
        "C:\\work space\\.twaco\\types\\check\\s0001.js(12,7): error TS2345: first line\r\n",
        "  The detailed reason\r\n",
        ".twaco/types/entities.d.ts(3,2): error TS1005: ';' expected.\n"
    ));
    assert_eq!(
        parsed,
        vec![
            CompilerFinding {
                file: "C:\\work space\\.twaco\\types\\check\\s0001.js".into(),
                line: 12,
                column: 7,
                code: "2345".into(),
                message: "first line The detailed reason".into(),
            },
            CompilerFinding {
                file: ".twaco/types/entities.d.ts".into(),
                line: 3,
                column: 2,
                code: "1005".into(),
                message: "';' expected.".into()
            },
        ]
    );
}

#[test]
fn maps_script_header_and_non_service_findings() {
    let solution: Solution = toml::from_str("[[project]]\nname = \"P\"\n").unwrap();
    let mut solution = solution;
    solution.root = PathBuf::from("C:/solution");
    let projects = vec![CheckProject {
        generated_name: "s0000.js".into(),
        script_path: solution.root.join("src/T/services/Run/script.js"),
        globals_path: solution.root.join("src/T/services/Run/twaco-globals.d.ts"),
        header_lines: 4,
    }];
    let finding = |file: &str, line| CompilerFinding {
        file: file.into(),
        line,
        column: 2,
        code: "1".into(),
        message: "x".into(),
    };
    assert_eq!(
        map_finding(&solution, &projects, &finding("s0000.js", 7)),
        ("src/T/services/Run/script.js".into(), 3, Some(0))
    );
    assert_eq!(
        map_finding(&solution, &projects, &finding("s0000.js", 2)),
        ("src/T/services/Run/twaco-globals.d.ts".into(), 2, Some(0))
    );
    assert_eq!(
        map_finding(
            &solution,
            &projects,
            &finding(".twaco/types/entities.d.ts", 9)
        ),
        (".twaco/types/entities.d.ts".into(), 9, None)
    );
}

#[test]
fn compiler_discovery_prefers_configuration_then_local_then_path() {
    let (root, mut solution) = fixture_solution("compiler-order", &[]);
    assert_eq!(
        compiler_command(&solution)[0],
        OsString::from(if cfg!(windows) { "tsc.cmd" } else { "tsc" })
    );

    let local = root.join("node_modules/typescript/bin/tsc");
    std::fs::create_dir_all(local.parent().unwrap()).unwrap();
    std::fs::write(&local, "").unwrap();
    assert_eq!(
        compiler_command(&solution),
        vec![OsString::from("node"), local.clone().into_os_string()]
    );

    solution.types.tsc = Some(vec!["custom-tsc".into(), "--flag".into()]);
    assert_eq!(
        compiler_command(&solution),
        vec![OsString::from("custom-tsc"), OsString::from("--flag")]
    );
    let _ = std::fs::remove_dir_all(root);
}

struct FakeCompiler {
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    calls: RefCell<Vec<(OsString, Vec<OsString>, PathBuf)>>,
}

impl CompilerRunner for FakeCompiler {
    fn run(
        &self,
        program: &OsStr,
        arguments: &[OsString],
        current_dir: &Path,
    ) -> std::io::Result<CompilerOutput> {
        self.calls.borrow_mut().push((
            program.to_os_string(),
            arguments.to_vec(),
            current_dir.to_path_buf(),
        ));
        Ok(CompilerOutput {
            success: self.success,
            stdout: self.stdout.clone(),
            stderr: self.stderr.clone(),
        })
    }
}

#[test]
fn compiler_errors_without_a_parsable_finding_are_a_broken_run() {
    let (root, solution) = fixture_solution("empty-compiler-error", &[]);
    let compiler = FakeCompiler {
        success: false,
        stdout: Vec::new(),
        stderr: b"\nnode: cannot find module typescript\nlong stack trace\n".to_vec(),
        calls: RefCell::new(Vec::new()),
    };
    let error = check_with(&solution, &compiler).unwrap_err().to_string();
    assert!(error.contains("non-zero without a parsable finding"));
    assert!(error.contains("ran:"));
    assert!(error.contains("--pretty"));
    assert!(error.contains("tsconfig.json"));
    assert!(error.contains("node: cannot find module typescript"));
    assert!(!error.contains("long stack trace"));
    assert!(error.contains("npm install --save-dev typescript"));
    let calls = compiler.calls.borrow();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].1.iter().any(|argument| argument == "--pretty"));
    assert_eq!(calls[0].2, root);
    let _ = std::fs::remove_dir_all(&calls[0].2);
}

#[test]
fn json_check_output_is_all_findings_and_no_summary() {
    let outcome = CheckOutcome {
        declarations: Outcome::default(),
        findings: vec![
            TypeFinding {
                file: "src/A/script.js".into(),
                line: 3,
                column: 7,
                code: "2345".into(),
                message: "first".into(),
            },
            TypeFinding {
                file: "src/B/script.js".into(),
                line: 9,
                column: 2,
                code: "1005".into(),
                message: "second".into(),
            },
        ],
        affected_services: 2,
        services: 4,
        elapsed: Duration::from_millis(1250),
    };
    let stdout: Vec<String> = outcome.findings.iter().map(finding_json).collect();
    let parsed: Vec<_> = stdout
        .iter()
        .filter_map(|line| crate::core::check::parse_finding("hook", line))
        .collect();

    assert_eq!(parsed.len(), outcome.findings.len());
    assert!(parsed.iter().all(|finding| finding.gate == "types"));
    assert_eq!(parsed[0].rule, "TS2345");
    assert_eq!(parsed[0].message, "first (column 7)");
    assert!(!stdout.iter().any(|line| line.starts_with("types:")));
    assert!(check_summary(&outcome).starts_with("types: 2 finding(s)"));
}
