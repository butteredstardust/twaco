use super::*;
use crate::core::commands::{self, Mode, Notices};

fn fixture() -> (tempfile::TempDir, Solution) {
    let dir = tempfile::Builder::new()
        .prefix("twaco-entity-edit-")
        .tempdir()
        .unwrap();
    std::fs::write(
        dir.path().join("twaco.toml"),
        "[solution]\nsrc = \"src\"\n[[project]]\nname = \"Acme.Orders\"\nroot = \"app\"\n",
    )
    .unwrap();
    for collection in ["Things", "ThingTemplates", "ThingShapes", "DataShapes"] {
        std::fs::create_dir_all(dir.path().join("app").join(collection)).unwrap();
    }
    let solution = Solution::load(&dir.path().join("twaco.toml")).unwrap();
    (dir, solution)
}

/// Create an entity with `twaco new`.
fn new(solution: &Solution, kind: Kind, name: &str, set: impl FnOnce(&mut NewRequest)) {
    let mut request = NewRequest::blank(kind, name);
    set(&mut request);
    let command = commands::entity_new::Request {
        request,
        mode: Mode::Apply,
        lock_label: "test new",
    };
    commands::entity_new::execute(solution, &command, &mut Notices::default()).unwrap();
}

fn plan_of(solution: &Solution, entity: &str, action: Action) -> Result<Plan, EditError> {
    plan(
        solution,
        &Request {
            entity: entity.into(),
            action,
        },
    )
}

fn run(solution: &Solution, entity: &str, action: Action) -> Plan {
    let request = commands::entity_edit::EditRequest {
        request: Request {
            entity: entity.into(),
            action,
        },
        mode: Mode::Apply,
        lock_label: "test edit",
    };
    commands::entity_edit::execute(solution, &request, &mut Notices::default())
        .unwrap()
        .plan
}

fn file(solution: &Solution, collection: &str, name: &str) -> String {
    let found = workspace::entities(solution);
    let entity = found
        .iter()
        .find(|file| file.info.collection == collection && file.info.name == name)
        .unwrap_or_else(|| panic!("{collection}/{name} exists"));
    String::from_utf8(std::fs::read(&entity.path).unwrap()).unwrap()
}

fn property(name: &str, base_type: &str) -> Property {
    Property {
        name: name.into(),
        base_type: Some(base_type.into()),
        ..Property::default()
    }
}

fn code(result: Result<Plan, EditError>) -> ErrorCode {
    result.expect_err("refused").code()
}

/// A template and two Things using it, plus a shape and a DataShape.
fn family(solution: &Solution) {
    new(solution, Kind::ThingShape, "Acme.Orders.Audit_TS", |_| {});
    new(
        solution,
        Kind::ThingTemplate,
        "Acme.Orders.Machine_TT",
        |_| {},
    );
    for thing in ["Acme.Orders.Press", "Acme.Orders.Saw"] {
        new(solution, Kind::Thing, thing, |request| {
            request.template = Some("Acme.Orders.Machine_TT".into());
        });
    }
    new(solution, Kind::DataShape, "Acme.Orders.Row_DS", |request| {
        request.fields = vec!["id:STRING:primary".into()];
    });
}

fn assert_in_step(solution: &Solution) {
    let sync = commands::sync::execute(
        solution,
        &commands::sync::SyncRequest {
            target: commands::sync::SyncTarget {
                project: None,
                entities: Vec::new(),
                all: true,
                reject_entities_with_all: false,
                missing_target: "name an entity, or pass --all",
            },
            mode: Mode::Plan,
            allow_structural: false,
            relayout: false,
            lock_label: "test sync",
        },
        &mut Notices::default(),
    )
    .unwrap();
    assert_eq!(sync.report.changed, 0, "{:?}", sync.report.log.lines);
    assert_eq!(sync.report.failed, 0, "{:?}", sync.report.log.lines);
    let failing: Vec<_> = crate::core::check::run(solution)
        .gates
        .into_iter()
        .filter(|gate| !gate.ok())
        .collect();
    assert!(failing.is_empty(), "{failing:?}");
}

const SPEED: &str = "
                    <PropertyDefinition
                     aspect.cacheTime=\"0.0\"
                     aspect.dataChangeType=\"VALUE\"
                     aspect.isLogged=\"true\"
                     aspect.isPersistent=\"true\"
                     aspect.isReadOnly=\"false\"
                     aspect.units=\"m/s\"
                     aspect.defaultValue=\"12.5\"
                     baseType=\"NUMBER\"
                     category=\"Motion\"
                     description=\"How fast\"
                     isLocalOnly=\"false\"
                     name=\"Speed\"
                     ordinal=\"1\"></PropertyDefinition>
                ";

#[test]
fn property_add_writes_the_mined_definition_and_nothing_else() {
    let (_dir, solution) = fixture();
    family(&solution);
    let before = file(&solution, "Things", "Acme.Orders.Press");
    let mut speed = property("Speed", "NUMBER");
    speed.default = Some("12.5".into());
    speed.persistent = Some(true);
    speed.logged = Some(true);
    speed.category = Some("Motion".into());
    speed.units = Some("m/s".into());
    speed.description = Some("How fast".into());
    let plan = run(&solution, "Acme.Orders.Press", Action::PropertyAdd(speed));
    assert_eq!(plan.changes.len(), 1);
    let after = file(&solution, "Things", "Acme.Orders.Press");
    // An empty section opens; the persistent property's stored value goes in too.
    let with_definition = before.replace(
        "<PropertyDefinitions></PropertyDefinitions>",
        &format!("<PropertyDefinitions>{SPEED}</PropertyDefinitions>"),
    );
    let stored = "<ThingProperties>\n                <Speed><Value>12.5</Value>\
                  <Timestamp>1970-01-01T00:00:00.000Z</Timestamp><Quality>UNKNOWN</Quality>\
                  </Speed>\n            </ThingProperties>";
    let stored = stored.replace("                  ", "");
    assert_eq!(
        after,
        with_definition.replace("<ThingProperties></ThingProperties>", &stored)
    );

    // The next one goes after it, with the next ordinal.
    let before = after;
    run(
        &solution,
        "Acme.Orders.Press",
        Action::PropertyAdd(property("Label", "STRING")),
    );
    let after = file(&solution, "Things", "Acme.Orders.Press");
    assert!(after.contains("name=\"Label\"\n                     ordinal=\"2\""));
    let label = Definition {
        base_type: "STRING".into(),
        default: None,
        persistent: false,
        logged: false,
        read_only: false,
        category: String::new(),
        units: None,
        data_shape: None,
        description: String::new(),
        name: "Label".into(),
        ordinal: "2".into(),
    };
    let anchor = "</PropertyDefinition>\n                </PropertyDefinitions>";
    let inserted = format!(
        "</PropertyDefinition>\n{}\n                </PropertyDefinitions>",
        label.xml()
    );
    assert_eq!(after, before.replacen(anchor, &inserted, 1));
}

#[test]
fn a_property_of_a_template_or_shape_gives_every_thing_that_has_it_a_stored_value() {
    let (_dir, solution) = fixture();
    family(&solution);
    let mut speed = property("Speed", "NUMBER");
    speed.persistent = Some(true);
    let plan = run(
        &solution,
        "Acme.Orders.Machine_TT",
        Action::PropertyAdd(speed),
    );
    let files: Vec<&str> = plan
        .changes
        .iter()
        .map(|change| change.entity.as_str())
        .collect();
    assert_eq!(
        files,
        [
            "ThingTemplates/Acme.Orders.Machine_TT",
            "Things/Acme.Orders.Press",
            "Things/Acme.Orders.Saw"
        ]
    );
    let template = file(&solution, "ThingTemplates", "Acme.Orders.Machine_TT");
    assert!(template.contains("name=\"Speed\""));
    assert!(!template.contains("<Speed>"), "a template stores no value");
    for thing in ["Acme.Orders.Press", "Acme.Orders.Saw"] {
        assert!(file(&solution, "Things", thing).contains("<Speed><Value>0.0</Value>"));
    }

    // A shape's persistent property reaches the Things once the template implements it.
    let mut label = property("Label", "STRING");
    label.persistent = Some(true);
    label.default = Some("a & b".into());
    let plan = run(
        &solution,
        "Acme.Orders.Audit_TS",
        Action::PropertyAdd(label),
    );
    assert_eq!(plan.changes.len(), 1, "nothing implements the shape yet");
    let plan = run(
        &solution,
        "Acme.Orders.Machine_TT",
        Action::ShapeAdd {
            shape: "Acme.Orders.Audit_TS".into(),
        },
    );
    assert_eq!(plan.changes.len(), 3);
    let press = file(&solution, "Things", "Acme.Orders.Press");
    assert!(press.contains("<Label><Value><![CDATA[a & b]]></Value>"));
    // In the server's order: Speed hashes to bucket 1, Label to 8.
    assert!(press.find("<Speed>").unwrap() < press.find("<Label>").unwrap());
    assert_in_step(&solution);

    // Stopping implementing it takes the values away again.
    run(
        &solution,
        "Acme.Orders.Machine_TT",
        Action::ShapeRemove {
            shape: "Acme.Orders.Audit_TS".into(),
            force: false,
        },
    );
    assert!(!file(&solution, "Things", "Acme.Orders.Saw").contains("<Label>"));
    assert!(file(&solution, "Things", "Acme.Orders.Saw").contains("<Speed>"));
}

#[test]
fn stored_values_follow_the_server_rules_for_each_type() {
    let stored = |base_type: &str, default: Option<&str>, persistent: bool| {
        Definition {
            base_type: base_type.into(),
            default: default.map(Into::into),
            persistent,
            logged: false,
            read_only: false,
            category: String::new(),
            units: None,
            data_shape: None,
            description: String::new(),
            name: "p".into(),
            ordinal: "1".into(),
        }
        .stored_value()
    };
    // As a 10.1 server wrote them (2026-10-16).
    let cases = [
        ("STRING", None, ""),
        ("STRING", Some("hello & bye"), "<![CDATA[hello & bye]]>"),
        ("TEXT", Some("multi"), "<![CDATA[multi]]>"),
        ("NUMBER", None, "0.0"),
        ("NUMBER", Some("7"), "7.0"),
        ("NUMBER", Some("12.5"), "12.5"),
        ("INTEGER", None, "0"),
        ("LONG", Some("9"), "9"),
        ("BOOLEAN", None, "false"),
        ("DATETIME", None, "1970-01-01T00:00:00.000Z"),
        ("LOCATION", None, "0.0,0.0,0.0"),
        (
            "JSON",
            Some("{\"a\":1}"),
            "<json><![CDATA[{\"a\":1}]]></json>",
        ),
        ("QUERY", None, "<json><![CDATA[{}]]></json>"),
        (
            "GUID",
            None,
            "<![CDATA[00000000-0000-0000-0000-000000000000]]>",
        ),
    ];
    for (base_type, default, value) in cases {
        assert_eq!(
            stored(base_type, default, true).unwrap().as_deref(),
            Some(value),
            "{base_type} {default:?}"
        );
    }
    assert_eq!(
        stored("STRING", Some("x"), false).unwrap(),
        None,
        "not persistent"
    );
    assert!(stored("INFOTABLE", None, true).is_err());
    // The order a 10.1 export wrote 22 values in.
    let written = [
        "pMASHUPNAME",
        "pLONG",
        "pLOCATION",
        "pQUERY",
        "pSTRING",
        "pBLOB",
        "pUSERNAME",
        "pIMAGELINK",
        "pINTEGER",
        "pDATETIME",
        "pPASSWORD",
        "pTEXT",
        "pTHINGNAME",
        "pBOOLEAN",
        "pGUID",
        "pINFOTABLE",
        "pJSON",
        "pHTML",
        "pHYPERLINK",
        "pTAGS",
        "pNUMBER",
        "pVEC2",
    ];
    let mut sorted: Vec<String> = written.iter().map(|name| name.to_string()).collect();
    sorted.sort();
    assert_eq!(java_hash_map_order(&sorted), written);
}

#[test]
fn property_add_refusals() {
    let (_dir, solution) = fixture();
    family(&solution);
    run(
        &solution,
        "Acme.Orders.Machine_TT",
        Action::PropertyAdd(property("Speed", "NUMBER")),
    );
    run(
        &solution,
        "Acme.Orders.Press",
        Action::PropertyAdd(property("Own", "STRING")),
    );
    let add = |entity: &str, property: Property| {
        plan_of(&solution, entity, Action::PropertyAdd(property))
    };
    assert_eq!(
        code(add("Acme.Orders.Press", property("Own", "STRING"))),
        ErrorCode::AlreadyExists
    );
    assert_eq!(
        code(add("Acme.Orders.Press", property("Speed", "STRING"))),
        ErrorCode::AlreadyExists
    );
    // The template cannot take a name a Thing using it already has: the server refuses it.
    let clash = add("Acme.Orders.Machine_TT", property("Own", "STRING"));
    assert_eq!(code(clash), ErrorCode::AlreadyExists);
    assert_eq!(
        code(add("Acme.Orders.Press", property("Odd", "WIDGET"))),
        ErrorCode::InvalidArguments
    );
    assert_eq!(
        code(add("Acme.Orders.Press", property("Rows", "INFOTABLE"))),
        ErrorCode::InvalidArguments
    );
    assert_eq!(
        code(add("Acme.Orders.Press", property("bad name", "STRING"))),
        ErrorCode::InvalidArguments
    );
    let mut bad_default = property("Count", "INTEGER");
    bad_default.default = Some("many".into());
    assert_eq!(
        code(add("Acme.Orders.Press", bad_default)),
        ErrorCode::InvalidArguments
    );
    let mut table = property("Rows", "INFOTABLE");
    table.data_shape = Some("Acme.Orders.Row_DS".into());
    table.persistent = Some(true);
    assert_eq!(
        code(add("Acme.Orders.Press", table)),
        ErrorCode::InvalidArguments
    );
    assert_eq!(
        code(add("Acme.Orders.Row_DS", property("Speed", "NUMBER"))),
        ErrorCode::InvalidArguments
    );
}

/// Add a service whose script reads `code` to `entity`'s file.
fn add_service(solution: &Solution, collection: &str, entity: &str, code: &str) {
    let found = workspace::entities(solution);
    let path = found
        .iter()
        .find(|file| file.info.collection == collection && file.info.name == entity)
        .unwrap()
        .path
        .clone();
    let xml = std::fs::read_to_string(&path).unwrap();
    let definition = "<ServiceDefinitions><ServiceDefinition aspect.isAsync=\"false\" \
        category=\"\" description=\"\" isAllowOverride=\"false\" isLocalOnly=\"false\" \
        isOpen=\"false\" isPrivate=\"false\" name=\"Report\"><ResultType baseType=\"NOTHING\" \
        description=\"\" name=\"result\" ordinal=\"0\"></ResultType><ParameterDefinitions>\
        </ParameterDefinitions></ServiceDefinition></ServiceDefinitions>";
    let implementation = format!(
        "<ServiceImplementations><ServiceImplementation description=\"\" handlerName=\"Script\" \
         name=\"Report\"><ConfigurationTables><ConfigurationTable dataShapeName=\"\" \
         description=\"\" isMultiRow=\"false\" name=\"Script\" ordinal=\"0\"><DataShape>\
         <FieldDefinitions><FieldDefinition baseType=\"STRING\" description=\"code\" name=\"code\" \
         ordinal=\"0\"></FieldDefinition></FieldDefinitions></DataShape><Rows><Row><code>\
         <![CDATA[{code}]]></code></Row></Rows></ConfigurationTable></ConfigurationTables>\
         </ServiceImplementation></ServiceImplementations>"
    );
    let xml = xml
        .replacen("<ServiceDefinitions></ServiceDefinitions>", definition, 1)
        .replacen(
            "<ServiceImplementations></ServiceImplementations>",
            &implementation,
            1,
        );
    std::fs::write(&path, xml).unwrap();
}

#[test]
fn a_property_in_use_is_not_removed_or_retyped_unless_forced() {
    let (_dir, solution) = fixture();
    family(&solution);
    let mut speed = property("Speed", "NUMBER");
    speed.persistent = Some(true);
    run(
        &solution,
        "Acme.Orders.Machine_TT",
        Action::PropertyAdd(speed),
    );
    run(
        &solution,
        "Acme.Orders.Machine_TT",
        Action::PropertyAdd(property("SpeedLimit", "NUMBER")),
    );
    // Saw inherits Speed and reads it; SpeedLimit is only a longer name.
    add_service(
        &solution,
        "Things",
        "Acme.Orders.Saw",
        "var s = me.Speed + me.SpeedLimit;",
    );
    let remove = |force: bool| {
        plan_of(
            &solution,
            "Acme.Orders.Machine_TT",
            Action::PropertyRemove {
                name: "Speed".into(),
                force,
            },
        )
    };
    let refused = remove(false).unwrap_err();
    assert_eq!(refused.code(), ErrorCode::GuardRefused);
    assert!(
        refused
            .to_string()
            .contains("Things/Acme.Orders.Saw (me.Speed)"),
        "{refused}"
    );
    let forced = remove(true).unwrap();
    assert!(
        forced.warnings[0].contains("Things/Acme.Orders.Saw"),
        "{:?}",
        forced.warnings
    );
    // Removing it also takes its stored values away.
    assert_eq!(forced.changes.len(), 3);
    assert!(forced
        .changes
        .iter()
        .skip(1)
        .all(|change| !String::from_utf8_lossy(&change.after).contains("<Speed>")));

    let mut retype = property("Speed", "STRING");
    retype.name = "Speed".into();
    let set = |force: bool| {
        plan_of(
            &solution,
            "Acme.Orders.Machine_TT",
            Action::PropertySet {
                property: retype.clone(),
                force,
            },
        )
    };
    assert_eq!(code(set(false)), ErrorCode::GuardRefused);
    let forced = set(true).unwrap();
    // A retype starts the stored values again, as the new type's default.
    let saw = forced
        .changes
        .iter()
        .find(|change| change.entity == "Things/Acme.Orders.Saw")
        .unwrap();
    assert!(String::from_utf8_lossy(&saw.after).contains("<Speed><Value></Value>"));
    // SpeedLimit is read too, as itself.
    let limit = plan_of(
        &solution,
        "Acme.Orders.Machine_TT",
        Action::PropertyRemove {
            name: "SpeedLimit".into(),
            force: false,
        },
    );
    assert_eq!(code(limit), ErrorCode::GuardRefused);
}

#[test]
fn only_a_whole_me_member_counts_as_a_read() {
    let script = |code: &str| format!("<Thing><code><![CDATA[{code}]]></code></Thing>");
    let reads = |code: &str, member: &str| reads_me_member(script(code).as_bytes(), member);
    assert!(reads("var s = me.Speed;", "Speed"));
    assert!(reads("me.Speed", "Speed"));
    assert!(!reads("var s = me.SpeedLimit;", "Speed"));
    assert!(!reads("var s = theme.Speed;", "Speed"));
    assert!(!reads("var s = other.me.Speed;", "Speed"));
    assert!(!reads("var s = me.speed;", "Speed"));
}

#[test]
fn property_set_changes_only_what_it_is_given() {
    let (_dir, solution) = fixture();
    family(&solution);
    let mut speed = property("Speed", "NUMBER");
    speed.default = Some("5".into());
    speed.logged = Some(true);
    run(&solution, "Acme.Orders.Press", Action::PropertyAdd(speed));
    let set = Property {
        name: "Speed".into(),
        category: Some("Motion".into()),
        logged: Some(false),
        ..Property::default()
    };
    run(
        &solution,
        "Acme.Orders.Press",
        Action::PropertySet {
            property: set,
            force: false,
        },
    );
    let press = file(&solution, "Things", "Acme.Orders.Press");
    assert!(press.contains("aspect.isLogged=\"false\""));
    assert!(press.contains("aspect.defaultValue=\"5.0\""));
    assert!(press.contains("category=\"Motion\""));
    assert!(
        !press.contains("<Speed>"),
        "not persistent, so no stored value"
    );
    // Made persistent, the Thing gets its stored value.
    let persist = Property {
        name: "Speed".into(),
        persistent: Some(true),
        ..Property::default()
    };
    run(
        &solution,
        "Acme.Orders.Press",
        Action::PropertySet {
            property: persist,
            force: false,
        },
    );
    assert!(file(&solution, "Things", "Acme.Orders.Press").contains("<Speed><Value>5.0</Value>"));
    let unknown = plan_of(
        &solution,
        "Acme.Orders.Press",
        Action::PropertySet {
            property: Property {
                name: "Nope".into(),
                ..Property::default()
            },
            force: false,
        },
    );
    assert_eq!(code(unknown), ErrorCode::UnknownEntity);
    assert_in_step(&solution);
}

#[test]
fn subscriptions_are_written_as_new_timer_writes_them() {
    let (_dir, solution) = fixture();
    family(&solution);
    run(
        &solution,
        "Acme.Orders.Press",
        Action::PropertyAdd(property("Speed", "NUMBER")),
    );
    let add = |name: &str, event: &str, property: Option<&str>| Action::SubscriptionAdd {
        name: name.into(),
        event: event.into(),
        source: None,
        source_property: property.map(Into::into),
        code: "logger.info(me.Speed);".into(),
    };
    let before = file(&solution, "Things", "Acme.Orders.Press");
    run(
        &solution,
        "Acme.Orders.Press",
        add("OnSpeed", "DataChange", Some("Speed")),
    );
    let after = file(&solution, "Things", "Acme.Orders.Press");
    let xml = entity_new::subscription_xml(
        "OnSpeed",
        "DataChange",
        "",
        "Speed",
        "logger.info(me.Speed);",
    );
    assert_eq!(
        after,
        before.replace(
            "<Subscriptions>\n                </Subscriptions>",
            &format!("<Subscriptions>\n{xml}\n                </Subscriptions>")
        )
    );
    let refused = |action| code(plan_of(&solution, "Acme.Orders.Press", action));
    assert_eq!(
        refused(add("Tick", "Timer", None)),
        ErrorCode::UnknownEntity
    );
    assert_eq!(
        refused(add("OnSpeed", "ThingStart", None)),
        ErrorCode::AlreadyExists
    );
    assert_eq!(
        refused(add("Watch", "DataChange", None)),
        ErrorCode::InvalidArguments
    );
    assert_eq!(
        refused(add("Watch", "DataChange", Some("Nope"))),
        ErrorCode::UnknownEntity
    );
    assert_eq!(
        refused(add("Start", "ThingStart", Some("Speed"))),
        ErrorCode::InvalidArguments
    );
    // Another Thing's event, and a timer's own.
    new(&solution, Kind::Timer, "Acme.Orders.Tick_TM", |request| {
        request.run_as = Some("Administrator".into());
    });
    let tick = Action::SubscriptionAdd {
        name: "OnTick".into(),
        event: "Timer".into(),
        source: Some("Acme.Orders.Tick_TM".into()),
        source_property: None,
        code: "me.Refresh();".into(),
    };
    run(&solution, "Acme.Orders.Saw", tick);
    assert!(file(&solution, "Things", "Acme.Orders.Saw").contains("source=\"Acme.Orders.Tick_TM\""));
    run(
        &solution,
        "Acme.Orders.Press",
        Action::SubscriptionRemove {
            name: "OnSpeed".into(),
        },
    );
    assert_eq!(file(&solution, "Things", "Acme.Orders.Press"), before);
    assert_in_step(&solution);
}

#[test]
fn shape_add_and_remove_with_their_refusals() {
    let (_dir, solution) = fixture();
    family(&solution);
    run(
        &solution,
        "Acme.Orders.Audit_TS",
        Action::PropertyAdd(property("Checked", "BOOLEAN")),
    );
    let before = file(&solution, "Things", "Acme.Orders.Press");
    let add = |entity: &str, shape: &str| {
        plan_of(
            &solution,
            entity,
            Action::ShapeAdd {
                shape: shape.into(),
            },
        )
    };
    assert_eq!(
        code(add("Acme.Orders.Press", "Acme.Orders.Missing_TS")),
        ErrorCode::UnknownEntity
    );
    run(
        &solution,
        "Acme.Orders.Press",
        Action::ShapeAdd {
            shape: "Acme.Orders.Audit_TS".into(),
        },
    );
    let after = file(&solution, "Things", "Acme.Orders.Press");
    assert_eq!(
        after,
        before.replace(
            "<ImplementedShapes>\n            </ImplementedShapes>",
            &format!(
                "<ImplementedShapes>\n{}\n            </ImplementedShapes>",
                entity_new::implemented_shape_xml("Acme.Orders.Audit_TS")
            )
        )
    );
    assert_eq!(
        code(add("Acme.Orders.Press", "Acme.Orders.Audit_TS")),
        ErrorCode::AlreadyExists
    );
    // Through the template: Saw would have it twice.
    run(
        &solution,
        "Acme.Orders.Press",
        Action::ShapeRemove {
            shape: "Acme.Orders.Audit_TS".into(),
            force: false,
        },
    );
    assert_eq!(file(&solution, "Things", "Acme.Orders.Press"), before);
    run(
        &solution,
        "Acme.Orders.Machine_TT",
        Action::ShapeAdd {
            shape: "Acme.Orders.Audit_TS".into(),
        },
    );
    assert_eq!(
        code(add("Acme.Orders.Saw", "Acme.Orders.Audit_TS")),
        ErrorCode::AlreadyExists
    );
    // A member clash: a shape whose property a Thing already has.
    new(&solution, Kind::ThingShape, "Acme.Orders.Other_TS", |_| {});
    run(
        &solution,
        "Acme.Orders.Other_TS",
        Action::PropertyAdd(property("Owned", "STRING")),
    );
    run(
        &solution,
        "Acme.Orders.Saw",
        Action::PropertyAdd(property("Owned", "STRING")),
    );
    assert_eq!(
        code(add("Acme.Orders.Saw", "Acme.Orders.Other_TS")),
        ErrorCode::AlreadyExists
    );
    assert_eq!(
        code(add("Acme.Orders.Audit_TS", "Acme.Orders.Other_TS")),
        ErrorCode::InvalidArguments
    );
    let not_there = plan_of(
        &solution,
        "Acme.Orders.Saw",
        Action::ShapeRemove {
            shape: "Acme.Orders.Audit_TS".into(),
            force: false,
        },
    );
    assert_eq!(
        code(not_there),
        ErrorCode::UnknownEntity,
        "it comes from the template"
    );
}

#[test]
fn timer_and_scheduler_set_change_only_their_settings_row() {
    let (_dir, solution) = fixture();
    let timer = |every: &'static str, disabled: bool| {
        move |request: &mut NewRequest| {
            request.run_as = Some("Administrator".into());
            request.every = Some(every.into());
            request.disabled = disabled;
        }
    };
    new(
        &solution,
        Kind::Timer,
        "Acme.Orders.Tick_TM",
        timer("60s", true),
    );
    run(
        &solution,
        "Acme.Orders.Tick_TM",
        Action::TimerSet {
            every: Some("5m".into()),
            run_as: None,
            enabled: Some(true),
        },
    );
    let edited = file(&solution, "Things", "Acme.Orders.Tick_TM");
    // The same as a timer created with those settings.
    let (_other, fresh) = fixture();
    new(
        &fresh,
        Kind::Timer,
        "Acme.Orders.Tick_TM",
        timer("5m", false),
    );
    assert_eq!(edited, file(&fresh, "Things", "Acme.Orders.Tick_TM"));

    new(
        &solution,
        Kind::Scheduler,
        "Acme.Orders.Night_SC",
        |request| {
            request.run_as = Some("Administrator".into());
        },
    );
    let before = file(&solution, "Things", "Acme.Orders.Night_SC");
    run(
        &solution,
        "Acme.Orders.Night_SC",
        Action::SchedulerSet {
            cron: Some("0 0 3 * * ?".into()),
            run_as: Some("Operator".into()),
            enabled: None,
        },
    );
    let after = file(&solution, "Things", "Acme.Orders.Night_SC");
    let expected = before
        .replace("<![CDATA[0 0/1 * * * ?]]>", "<![CDATA[0 0 3 * * ?]]>")
        .replace(
            "<runAsUser><![CDATA[Administrator]]>",
            "<runAsUser><![CDATA[Operator]]>",
        );
    assert_eq!(after, expected);
    let refused = |entity: &str, action| code(plan_of(&solution, entity, action));
    let nothing = Action::TimerSet {
        every: None,
        run_as: None,
        enabled: None,
    };
    assert_eq!(
        refused("Acme.Orders.Tick_TM", nothing),
        ErrorCode::InvalidArguments
    );
    let wrong = Action::TimerSet {
        every: Some("1s".into()),
        run_as: None,
        enabled: None,
    };
    assert_eq!(
        refused("Acme.Orders.Night_SC", wrong),
        ErrorCode::InvalidArguments
    );
    let bad_cron = Action::SchedulerSet {
        cron: Some("every day".into()),
        run_as: None,
        enabled: None,
    };
    assert_eq!(
        refused("Acme.Orders.Night_SC", bad_cron),
        ErrorCode::InvalidArguments
    );
    assert_in_step(&solution);
}

#[test]
fn style_media_menu_and_state_edits_match_what_new_writes() {
    let (_dir, solution) = fixture();
    let (_other, fresh) = fixture();
    for target in [&solution, &fresh] {
        std::fs::create_dir_all(target.root.join("app/Mashups")).unwrap();
    }

    // Style: only the given keys change.
    new(&solution, Kind::Style, "Acme.Orders.Good_SD", |request| {
        request.background = Some("#66cc33".into());
    });
    let change = StyleChange {
        foreground: Some("#000000".into()),
        bold: Some(true),
        ..StyleChange::default()
    };
    run(&solution, "Acme.Orders.Good_SD", Action::StyleSet(change));
    new(&fresh, Kind::Style, "Acme.Orders.Good_SD", |request| {
        request.background = Some("#66cc33".into());
        request.foreground = Some("#000000".into());
        request.bold = true;
    });
    assert_eq!(
        file(&solution, "StyleDefinitions", "Acme.Orders.Good_SD"),
        file(&fresh, "StyleDefinitions", "Acme.Orders.Good_SD")
    );
    let bad = StyleChange {
        text_size: Some("huge".into()),
        ..StyleChange::default()
    };
    assert_eq!(
        code(plan_of(
            &solution,
            "Acme.Orders.Good_SD",
            Action::StyleSet(bad)
        )),
        ErrorCode::InvalidArguments
    );

    // State: the states replaced, as new state writes them.
    let states = vec![
        "<5=Low;background:#66cc33".to_string(),
        "*=High".to_string(),
    ];
    new(&solution, Kind::State, "Acme.Orders.Level_SD", |request| {
        request.state_type = Some("numeric".into());
        request.states = vec!["*=Any".into()];
    });
    run(
        &solution,
        "Acme.Orders.Level_SD",
        Action::StateSet {
            state_type: "numeric".into(),
            states: states.clone(),
        },
    );
    new(&fresh, Kind::State, "Acme.Orders.Level_SD", |request| {
        request.state_type = Some("numeric".into());
        request.states = states.clone();
    });
    assert_eq!(
        file(&solution, "StateDefinitions", "Acme.Orders.Level_SD"),
        file(&fresh, "StateDefinitions", "Acme.Orders.Level_SD")
    );

    // Menu: an item added equals a menu made with both; removed, the one-item menu again.
    let one = |request: &mut NewRequest| {
        request.items = vec!["Home=Acme.Orders.Home_MU".into()];
        request.default = true;
    };
    new(&solution, Kind::Menu, "Acme.Orders.Main_MN", one);
    let single = file(&solution, "Menus", "Acme.Orders.Main_MN");
    run(
        &solution,
        "Acme.Orders.Main_MN",
        Action::MenuItemAdd {
            title: "Orders".into(),
            mashup: "Acme.Orders.Orders_MU".into(),
            default: false,
            before: None,
        },
    );
    new(&fresh, Kind::Menu, "Acme.Orders.Main_MN", |request| {
        request.items = vec![
            "Home=Acme.Orders.Home_MU".into(),
            "Orders=Acme.Orders.Orders_MU".into(),
        ];
        request.default = true;
    });
    assert_eq!(
        file(&solution, "Menus", "Acme.Orders.Main_MN"),
        file(&fresh, "Menus", "Acme.Orders.Main_MN")
    );
    run(
        &solution,
        "Acme.Orders.Main_MN",
        Action::MenuItemRemove {
            title: "Orders".into(),
        },
    );
    assert_eq!(file(&solution, "Menus", "Acme.Orders.Main_MN"), single);
    let duplicate = Action::MenuItemAdd {
        title: "Home".into(),
        mashup: "Acme.Orders.Home_MU".into(),
        default: false,
        before: None,
    };
    assert_eq!(
        code(plan_of(&solution, "Acme.Orders.Main_MN", duplicate)),
        ErrorCode::AlreadyExists
    );
    let missing_anchor = Action::MenuItemAdd {
        title: "New".into(),
        mashup: "Acme.Orders.Home_MU".into(),
        default: false,
        before: Some("Nope".into()),
    };
    assert_eq!(
        code(plan_of(&solution, "Acme.Orders.Main_MN", missing_anchor)),
        ErrorCode::UnknownEntity
    );

    // Media: the content replaced, the rest kept.
    let png = solution.root.join("one.png");
    let gif = solution.root.join("one.gif");
    std::fs::write(&png, b"\x89PNG\r\n\x1a\nrest").unwrap();
    std::fs::write(&gif, b"GIF89a....").unwrap();
    new(&solution, Kind::Media, "Acme.Orders.Logo_MD", |request| {
        request.file = Some(png.clone());
    });
    let before = file(&solution, "MediaEntities", "Acme.Orders.Logo_MD");
    run(
        &solution,
        "Acme.Orders.Logo_MD",
        Action::MediaSet { file: gif.clone() },
    );
    let after = file(&solution, "MediaEntities", "Acme.Orders.Logo_MD");
    let encode = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
    use base64::Engine;
    assert_eq!(
        after,
        before.replace(&encode(b"\x89PNG\r\n\x1a\nrest"), &encode(b"GIF89a...."))
    );
    let text = solution.root.join("notes.txt");
    std::fs::write(&text, "words").unwrap();
    assert_eq!(
        code(plan_of(
            &solution,
            "Acme.Orders.Logo_MD",
            Action::MediaSet { file: text }
        )),
        ErrorCode::InvalidArguments
    );
    assert_in_step(&solution);
}

#[test]
fn a_self_closing_content_element_is_opened_up() {
    let (_dir, solution) = fixture();
    new(&solution, Kind::Style, "Acme.Orders.Good_SD", |_| {});
    let path = workspace::entities(&solution)
        .into_iter()
        .find(|file| file.info.name == "Acme.Orders.Good_SD")
        .unwrap()
        .path;
    let xml = std::fs::read_to_string(&path).unwrap();
    let start = xml.find("<content>").unwrap();
    let end = xml.find("</content>").unwrap() + "</content>".len();
    let empty = format!("{}<content/>{}", &xml[..start], &xml[end..]);
    std::fs::write(&path, &empty).unwrap();
    // An empty content holds no JSON to merge into: refused, never written broken.
    let change = StyleChange {
        bold: Some(true),
        ..StyleChange::default()
    };
    assert_eq!(
        code(plan_of(
            &solution,
            "Acme.Orders.Good_SD",
            Action::StyleSet(change)
        )),
        ErrorCode::InvalidArguments
    );
    // A state set replaces the content outright, so it opens the element.
    new(&solution, Kind::State, "Acme.Orders.Level_SD", |request| {
        request.state_type = Some("numeric".into());
        request.states = vec!["*=Any".into()];
    });
    let path = workspace::entities(&solution)
        .into_iter()
        .find(|file| file.info.name == "Acme.Orders.Level_SD")
        .unwrap()
        .path;
    let xml = std::fs::read_to_string(&path).unwrap();
    let start = xml.find("<content>").unwrap();
    let end = xml.find("</content>").unwrap() + "</content>".len();
    std::fs::write(&path, format!("{}<content/>{}", &xml[..start], &xml[end..])).unwrap();
    run(
        &solution,
        "Acme.Orders.Level_SD",
        Action::StateSet {
            state_type: "numeric".into(),
            states: vec!["*=Any".into()],
        },
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), xml);
}

#[test]
fn a_menu_item_edit_keeps_every_other_item_as_it_is() {
    let (_dir, solution) = fixture();
    std::fs::create_dir_all(solution.root.join("app/Mashups")).unwrap();
    new(&solution, Kind::Menu, "Acme.Orders.Main_MN", |request| {
        request.items = vec!["Home=Acme.Orders.Home_MU".into()];
        request.default = true;
    });
    let path = workspace::entities(&solution)
        .into_iter()
        .find(|file| file.info.name == "Acme.Orders.Main_MN")
        .unwrap()
        .path;
    // An item with its own settings and a group, as Composer writes one.
    let xml = std::fs::read_to_string(&path).unwrap();
    let custom = xml
        .replacen("description=\"\" imageURL=\"\" isDefault=\"true\"", "description=\"Start\" imageURL=\"/home.png\" isDefault=\"true\"", 1)
        .replacen("linkTarget=\"Self\" linkType=\"Mashup\" title=\"Home\"", "linkTarget=\"Popup\" linkType=\"Mashup\" title=\"Home\"", 1)
        .replacen(
            "<GroupReferences><Members></Members></GroupReferences>\n                </MenuItem>",
            "<GroupReferences><Members><Member name=\"Acme.Users_UG\" type=\"Group\"></Member></Members></GroupReferences>\n                </MenuItem>",
            1,
        );
    assert_ne!(custom, xml);
    std::fs::write(&path, &custom).unwrap();
    let home_item = |text: &str| {
        let start = text.find("<MenuItem ").unwrap();
        let end = text.find("</MenuItem>").unwrap();
        text[start..end].to_string()
    };
    run(
        &solution,
        "Acme.Orders.Main_MN",
        Action::MenuItemAdd {
            title: "Orders".into(),
            mashup: "Acme.Orders.Orders_MU".into(),
            default: true,
            before: None,
        },
    );
    let after = std::fs::read_to_string(&path).unwrap();
    // Home keeps its settings and its group; only isDefault moves to the new item.
    assert_eq!(
        home_item(&after),
        home_item(&custom).replace("isDefault=\"true\"", "isDefault=\"false\"")
    );
    assert!(after.contains("isDefault=\"true\" linkDestination=\"Acme.Orders.Orders_MU\""));
    // Home's row repeats its settings.
    assert!(
        after.contains("<description><![CDATA[Start]]></description>"),
        "{after}"
    );
    assert!(after.contains("<imageURL><![CDATA[/home.png]]></imageURL>"));
    assert!(after.contains("<linkTarget><![CDATA[Popup]]></linkTarget>"));
    assert!(after.contains("<menuId><![CDATA[Acme.Orders.Main_MN_1]]></menuId>"));
    run(
        &solution,
        "Acme.Orders.Main_MN",
        Action::MenuItemRemove {
            title: "Orders".into(),
        },
    );
    let back = std::fs::read_to_string(&path).unwrap();
    assert_eq!(home_item(&back), home_item(&after));
}

#[test]
fn property_set_leaves_aspects_it_does_not_model_alone() {
    let (_dir, solution) = fixture();
    family(&solution);
    run(
        &solution,
        "Acme.Orders.Press",
        Action::PropertyAdd(property("Speed", "NUMBER")),
    );
    let path = workspace::entities(&solution)
        .into_iter()
        .find(|file| file.info.name == "Acme.Orders.Press")
        .unwrap()
        .path;
    // An aspect twaco does not model, as a remote property carries.
    let xml = std::fs::read_to_string(&path).unwrap().replacen(
        "aspect.dataChangeType=\"VALUE\"",
        "aspect.dataChangeType=\"VALUE\"\n                     aspect.pushType=\"ALWAYS\"",
        1,
    );
    std::fs::write(&path, &xml).unwrap();
    let set = Property {
        name: "Speed".into(),
        description: Some("How fast".into()),
        default: Some("3".into()),
        ..Property::default()
    };
    run(
        &solution,
        "Acme.Orders.Press",
        Action::PropertySet {
            property: set,
            force: false,
        },
    );
    let after = std::fs::read_to_string(&path).unwrap();
    let expected = xml
        .replacen("description=\"\"\n                     isLocalOnly=\"false\"\n                     name=\"Speed\"", "description=\"How fast\"\n                     isLocalOnly=\"false\"\n                     name=\"Speed\"", 1)
        .replacen("aspect.isReadOnly=\"false\"\n                     baseType=\"NUMBER\"", "aspect.isReadOnly=\"false\"\n                     aspect.defaultValue=\"3.0\"\n                     baseType=\"NUMBER\"", 1);
    assert_eq!(after, expected);
    // Clearing the default removes just that attribute.
    let clear = Property {
        name: "Speed".into(),
        no_default: true,
        ..Property::default()
    };
    run(
        &solution,
        "Acme.Orders.Press",
        Action::PropertySet {
            property: clear,
            force: false,
        },
    );
    let cleared = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        cleared,
        expected.replacen("\n                     aspect.defaultValue=\"3.0\"", "", 1)
    );
}

#[test]
fn a_shape_whose_members_are_used_is_not_removed_unless_forced() {
    let (_dir, solution) = fixture();
    family(&solution);
    run(
        &solution,
        "Acme.Orders.Audit_TS",
        Action::PropertyAdd(property("Checked", "BOOLEAN")),
    );
    run(
        &solution,
        "Acme.Orders.Press",
        Action::ShapeAdd {
            shape: "Acme.Orders.Audit_TS".into(),
        },
    );
    add_service(
        &solution,
        "Things",
        "Acme.Orders.Press",
        "var done = me.Checked;",
    );
    let remove = |force: bool| {
        plan_of(
            &solution,
            "Acme.Orders.Press",
            Action::ShapeRemove {
                shape: "Acme.Orders.Audit_TS".into(),
                force,
            },
        )
    };
    let refused = remove(false).unwrap_err();
    assert_eq!(refused.code(), ErrorCode::GuardRefused);
    assert!(refused.to_string().contains("me.Checked"), "{refused}");
    let forced = remove(true).unwrap();
    assert!(
        forced.warnings[0].contains("Checked"),
        "{:?}",
        forced.warnings
    );
}

#[test]
fn a_shape_member_used_through_an_inheriting_thing_is_guarded() {
    let (_dir, solution) = fixture();
    family(&solution);
    run(
        &solution,
        "Acme.Orders.Audit_TS",
        Action::PropertyAdd(property("Checked", "BOOLEAN")),
    );
    run(
        &solution,
        "Acme.Orders.Machine_TT",
        Action::ShapeAdd {
            shape: "Acme.Orders.Audit_TS".into(),
        },
    );
    // Press reads the property Saw has through the template and the shape.
    add_service(
        &solution,
        "Things",
        "Acme.Orders.Press",
        "var done = Things[\"Acme.Orders.Saw\"].Checked;",
    );
    let refused = plan_of(
        &solution,
        "Acme.Orders.Machine_TT",
        Action::ShapeRemove {
            shape: "Acme.Orders.Audit_TS".into(),
            force: false,
        },
    )
    .unwrap_err();
    assert_eq!(refused.code(), ErrorCode::GuardRefused);
    assert!(
        refused.to_string().contains("Things/Acme.Orders.Press"),
        "{refused}"
    );
}

#[test]
fn a_crlf_menu_keeps_its_line_endings_and_a_terminator_in_an_item_is_refused() {
    let (_dir, solution) = fixture();
    std::fs::create_dir_all(solution.root.join("app/Mashups")).unwrap();
    new(&solution, Kind::Menu, "Acme.Orders.Main_MN", |request| {
        request.items = vec!["Home=Acme.Orders.Home_MU".into()];
    });
    let path = workspace::entities(&solution)
        .into_iter()
        .find(|file| file.info.name == "Acme.Orders.Main_MN")
        .unwrap()
        .path;
    let lf = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, lf.replace('\n', "\r\n")).unwrap();
    let add = Action::MenuItemAdd {
        title: "Orders".into(),
        mashup: "Acme.Orders.Orders_MU".into(),
        default: false,
        before: Some("Home".into()),
    };
    run(&solution, "Acme.Orders.Main_MN", add);
    run(
        &solution,
        "Acme.Orders.Main_MN",
        Action::MenuItemRemove {
            title: "Home".into(),
        },
    );
    let after = std::fs::read_to_string(&path).unwrap();
    assert!(after.contains("title=\"Orders\""));
    assert_eq!(after.matches('\n').count(), after.matches("\r\n").count());
    // An existing title holding `]]>` cannot be repeated in a CDATA row.
    std::fs::write(
        &path,
        after.replace("title=\"Orders\"", "title=\"A]]&gt;\""),
    )
    .unwrap();
    let again = Action::MenuItemAdd {
        title: "More".into(),
        mashup: "Acme.Orders.Orders_MU".into(),
        default: false,
        before: None,
    };
    assert_eq!(
        code(plan_of(&solution, "Acme.Orders.Main_MN", again)),
        ErrorCode::InvalidArguments
    );
}

#[test]
fn a_crlf_file_keeps_its_line_endings() {
    let (_dir, solution) = fixture();
    family(&solution);
    let path = workspace::entities(&solution)
        .into_iter()
        .find(|file| file.info.name == "Acme.Orders.Press")
        .unwrap()
        .path;
    let lf = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, lf.replace('\n', "\r\n")).unwrap();
    let mut speed = property("Speed", "NUMBER");
    speed.persistent = Some(true);
    run(&solution, "Acme.Orders.Press", Action::PropertyAdd(speed));
    let after = std::fs::read_to_string(&path).unwrap();
    assert!(after.contains("name=\"Speed\""));
    assert_eq!(after.matches('\n').count(), after.matches("\r\n").count());
}

#[test]
fn a_plan_writes_nothing() {
    let (_dir, solution) = fixture();
    family(&solution);
    let snapshot = |solution: &Solution| -> Vec<(PathBuf, Vec<u8>)> {
        let mut files: Vec<_> = walk(&solution.root)
            .into_iter()
            .map(|path| {
                let bytes = std::fs::read(&path).unwrap();
                (path, bytes)
            })
            .collect();
        files.sort();
        files
    };
    let before = snapshot(&solution);
    let mut speed = property("Speed", "NUMBER");
    speed.persistent = Some(true);
    let plan = plan_of(
        &solution,
        "Acme.Orders.Machine_TT",
        Action::PropertyAdd(speed),
    )
    .unwrap();
    assert_eq!(plan.changes.len(), 3);
    assert_eq!(snapshot(&solution), before);
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}
