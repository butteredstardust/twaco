use super::*;

/// A Thing with script services: (name, parameters XML, result XML, script).
fn thing(name: &str, services: &[(&str, &str, &str, &str)]) -> String {
    let definitions: String = services
        .iter()
        .map(|(service, parameters, result, _)| {
            format!(
                "<ServiceDefinition aspect.isAsync=\"false\" category=\"\" description=\"\" \
                 isAllowOverride=\"false\" isLocalOnly=\"false\" isOpen=\"false\" isPrivate=\"false\" \
                 name=\"{service}\">{result}<ParameterDefinitions>{parameters}</ParameterDefinitions>\
                 </ServiceDefinition>"
            )
        })
        .collect();
    let implementations: String = services
        .iter()
        .map(|(service, _, _, code)| {
            format!(
                "<ServiceImplementation description=\"\" handlerName=\"Script\" name=\"{service}\">\
                 <ConfigurationTables><ConfigurationTable dataShapeName=\"\" description=\"\" \
                 isMultiRow=\"false\" name=\"Script\" ordinal=\"0\"><DataShape><FieldDefinitions>\
                 <FieldDefinition baseType=\"STRING\" description=\"code\" name=\"code\" ordinal=\"0\">\
                 </FieldDefinition></FieldDefinitions></DataShape><Rows><Row><code><![CDATA[{code}]]>\
                 </code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation>"
            )
        })
        .collect();
    format!(
        "<Entities><Things><Thing name=\"{name}\" projectName=\"Acme.Orders\" \
         thingTemplate=\"GenericThing\"><ThingShape><PropertyDefinitions></PropertyDefinitions>\
         <ServiceDefinitions>{definitions}</ServiceDefinitions><EventDefinitions></EventDefinitions>\
         <ServiceMappings></ServiceMappings><ServiceImplementations>{implementations}\
         </ServiceImplementations><Subscriptions></Subscriptions></ThingShape></Thing></Things>\
         </Entities>"
    )
}

/// A board that calls GetOrders as `Orders` on load, binds `id` from a text field and leaves
/// `limit` unbound, and sends the rows to a grid and a selected total to a label.
fn board() -> String {
    let content = serde_json::json!({
        "Data": {
            "Things_Acme.Orders.Manager": {
                "EntityName": "Acme.Orders.Manager",
                "EntityType": "Things",
                "Services": [{"Name": "Orders", "Target": "GetOrders", "Parameters": {}}]
            }
        },
        "DataBindings": [
            {"Id": "b1", "SourceArea": "Data", "SourceSection": "Things_Acme.Orders.Manager",
             "SourceId": "Orders", "SourceDetails": "AllData", "TargetArea": "UI",
             "TargetId": "ptcsgrid-3", "TargetSection": "",
             "PropertyMaps": [{"SourceProperty": "", "TargetProperty": "Data"}]},
            {"Id": "b2", "SourceArea": "Data", "SourceSection": "Things_Acme.Orders.Manager",
             "SourceId": "Orders", "SourceDetails": "SelectedRows", "TargetArea": "UI",
             "TargetId": "ptcslabel-5", "TargetSection": "",
             "PropertyMaps": [{"SourceProperty": "Total", "TargetProperty": "LabelText"}]},
            {"Id": "b3", "SourceArea": "UI", "SourceId": "ptcstextfield-4", "SourceSection": "",
             "TargetArea": "Data", "TargetSection": "Things_Acme.Orders.Manager",
             "TargetId": "Orders",
             "PropertyMaps": [{"SourceProperty": "Text", "TargetProperty": "id"}]}
        ],
        "Events": [
            {"Id": "e1", "EventTriggerArea": "Mashup", "EventTriggerId": "",
             "EventTriggerEvent": "Loaded", "EventTriggerSection": "",
             "EventHandlerArea": "Data", "EventHandlerId": "Things_Acme.Orders.Manager",
             "EventHandlerService": "Orders"}
        ],
        "UI": {
            "Properties": {"Id": "mashup-root", "Type": "mashup", "DisplayName": "Mashup"},
            "Widgets": [
                {"Properties": {"Id": "ptcsgrid-3", "Type": "ptcsgrid", "DisplayName": "grid"}},
                {"Properties": {"Id": "ptcstextfield-4", "Type": "ptcstextfield",
                                "DisplayName": "field", "Text": ""}},
                {"Properties": {"Id": "ptcslabel-5", "Type": "ptcslabel", "DisplayName": "label"}}
            ]
        },
        "mashupType": "mashup"
    });
    format!(
        "<Entities><Mashups><Mashup name=\"Acme.Orders.Board_MU\" projectName=\"Acme.Orders\">\
         <ParameterDefinitions></ParameterDefinitions><mashupContent><![CDATA[{content}]]>\
         </mashupContent></Mashup></Mashups></Entities>"
    )
}

fn fixture() -> (tempfile::TempDir, Solution) {
    let dir = tempfile::Builder::new()
        .prefix("twaco-trace-")
        .tempdir()
        .unwrap();
    let root = dir.path();
    std::fs::write(
        root.join("twaco.toml"),
        "[[project]]\nname = \"Acme.Orders\"\nroot = \".\"\n",
    )
    .unwrap();
    for collection in ["Things", "Mashups"] {
        std::fs::create_dir_all(root.join(collection)).unwrap();
    }
    let parameters = "<FieldDefinition aspect.defaultValue=\"x\" baseType=\"STRING\" \
                      description=\"\" name=\"id\" ordinal=\"0\"></FieldDefinition>\
                      <FieldDefinition baseType=\"INTEGER\" description=\"\" name=\"limit\" \
                      ordinal=\"1\" aspect.defaultValue=\"50\"></FieldDefinition>";
    let rows = "<ResultType aspect.dataShape=\"Acme.Orders.Order_DS\" baseType=\"INFOTABLE\" \
                description=\"\" name=\"result\" ordinal=\"0\"></ResultType>";
    let nothing = "<ResultType baseType=\"NOTHING\" description=\"\" name=\"result\" \
                   ordinal=\"0\"></ResultType>";
    let manager = thing(
        "Acme.Orders.Manager",
        &[
            (
                "GetOrders",
                parameters,
                rows,
                "Things[\"Acme.Orders.Audit\"].Record();\nresult = me.Query();",
            ),
            ("Query", "", rows, "result = null;"),
            (
                "Wrap",
                "",
                nothing,
                "var mine = me.GetOrders({ id: \"w\" });\nlogger.info(mine.length);",
            ),
        ],
    );
    let report = thing(
        "Acme.Orders.Report",
        &[(
            "Build",
            "",
            nothing,
            "var x = 1;\nvar orders = Things[\"Acme.Orders.Manager\"].GetOrders({\n    id: \"a\",\n    limit: 5\n});\nvar total = orders.rows[0].Total + orders.getRow(0).Count + orders.length;",
        )],
    );
    let audit = thing(
        "Acme.Orders.Audit",
        &[("Record", "", nothing, "logger.info(1);")],
    );
    for (name, xml) in [
        ("Acme.Orders.Manager", manager),
        ("Acme.Orders.Report", report),
        ("Acme.Orders.Audit", audit),
    ] {
        std::fs::write(root.join(format!("Things/{name}.xml")), xml).unwrap();
    }
    std::fs::write(root.join("Mashups/Acme.Orders.Board_MU.xml"), board()).unwrap();
    let solution = Solution::load(&root.join("twaco.toml")).unwrap();
    (dir, solution)
}

#[test]
fn a_trace_shows_how_each_caller_uses_the_service() {
    let (_dir, solution) = fixture();
    let trace = run(&solution, "Acme.Orders.Manager.GetOrders", None).unwrap();
    assert_eq!(trace.service, "Things/Acme.Orders.Manager.GetOrders");
    assert_eq!(trace.returns, "INFOTABLE Acme.Orders.Order_DS");
    let takes: Vec<(&str, Option<&str>)> = trace
        .parameters
        .iter()
        .map(|p| (p.name.as_str(), p.default.as_deref()))
        .collect();
    assert_eq!(takes, [("id", Some("x")), ("limit", Some("50"))]);

    assert_eq!(trace.mashups.len(), 1, "{trace:?}");
    let board = &trace.mashups[0];
    assert_eq!(board.mashup, "Mashups/Acme.Orders.Board_MU");
    assert_eq!(board.runs_on, ["mashup:Loaded"]);
    assert_eq!(
        board.parameters,
        [
            (
                "id".to_string(),
                vec!["ui:ptcstextfield-4.Text".to_string()]
            ),
            (
                "limit".to_string(),
                vec!["unbound (default 50)".to_string()]
            ),
        ]
    );
    assert!(board
        .results
        .contains(&("(all data)".into(), "ui:ptcsgrid-3.Data".into())));
    assert!(board
        .results
        .iter()
        .any(|(path, to)| path.ends_with("Total") && to == "ui:ptcslabel-5.LabelText"));

    let calls: Vec<(&str, usize)> = trace
        .scripts
        .iter()
        .map(|call| (call.caller.as_str(), call.line))
        .collect();
    assert_eq!(
        calls,
        [
            ("Things/Acme.Orders.Manager.Wrap", 1),
            ("Things/Acme.Orders.Report.Build", 2),
        ]
    );
    let build = &trace.scripts[1];
    assert_eq!(
        build.call,
        "Things[\"Acme.Orders.Manager\"].GetOrders({ id: \"a\", limit: 5 })"
    );
    assert_eq!(build.into.as_deref(), Some("orders"));
    assert_eq!(
        build.reads,
        ["getRow().Count", "length", "rows", "rows[].Total"]
    );
    assert_eq!(trace.scripts[0].reads, ["length"]);

    assert_eq!(
        trace.calls,
        [
            "Things/Acme.Orders.Audit.Record",
            "Things/Acme.Orders.Manager.Query"
        ]
    );
    assert!(render(&trace).contains("limit <- unbound (default 50)"));
}

#[test]
fn param_narrows_and_errors_say_what_is_wrong() {
    let (_dir, solution) = fixture();
    let trace = run(&solution, "Acme.Orders.Manager.GetOrders", Some("limit")).unwrap();
    assert_eq!(trace.parameters.len(), 1);
    assert_eq!(trace.mashups[0].parameters.len(), 1);
    assert!(matches!(
        run(&solution, "Acme.Orders.Manager.Missing", None),
        Err(TraceError::NoService { .. })
    ));
    assert!(matches!(
        run(&solution, "nodot", None),
        Err(TraceError::Shape)
    ));
    assert!(matches!(
        run(&solution, "Acme.Orders.Nobody.GetOrders", None),
        Err(TraceError::Entity(_))
    ));
}

#[test]
fn a_caller_through_thing_templates_is_found_and_a_rebound_variable_is_not_followed() {
    let (dir, solution) = fixture();
    let nothing = "<ResultType baseType=\"NOTHING\" description=\"\" name=\"result\" \
                   ordinal=\"0\"></ResultType>";
    // A template with a service, and a Thing of the same name as the traced Thing's template
    // namesake: only the right collection counts.
    std::fs::create_dir_all(dir.path().join("ThingTemplates")).unwrap();
    let template = thing(
        "Acme.Orders.Base_TT",
        &[("Describe", "", nothing, "result = 1;")],
    )
    .replace("<Things>", "<ThingTemplates>")
    .replace("</Things>", "</ThingTemplates>")
    .replace("<Thing ", "<ThingTemplate ")
    .replace("</Thing>", "</ThingTemplate>")
    .replace(
        " thingTemplate=\"GenericThing\"",
        " baseThingTemplate=\"GenericThing\"",
    );
    std::fs::write(
        dir.path().join("ThingTemplates/Acme.Orders.Base_TT.xml"),
        template,
    )
    .unwrap();
    let caller = thing(
        "Acme.Orders.Batch",
        &[
            (
                "Run",
                "",
                nothing,
                "var rows = ThingTemplates[\"Acme.Orders.Base_TT\"].Describe();\n\
                 function helper(rows) { return rows.Secret; }\n\
                 rows = other();\n\
                 var total = rows.rows[0].Total;",
            ),
            (
                "Elsewhere",
                "",
                nothing,
                "var r = ThingTemplates[\"Acme.Orders.Manager\"].GetOrders();",
            ),
        ],
    );
    std::fs::write(dir.path().join("Things/Acme.Orders.Batch.xml"), caller).unwrap();
    let solution = Solution::load(&solution.root.join("twaco.toml")).unwrap();
    let trace = run(&solution, "Acme.Orders.Base_TT.Describe", None).unwrap();
    let batch = trace
        .scripts
        .iter()
        .find(|call| call.caller == "Things/Acme.Orders.Batch.Run")
        .expect("a ThingTemplates[...] caller is found");
    assert!(!batch.reads_followed);
    assert!(batch.reads.is_empty());
    assert!(render(&trace).contains("reads not followed"));
    // ThingTemplates["Acme.Orders.Manager"] names no template: not a caller of the Thing.
    let orders = run(&solution, "Acme.Orders.Manager.GetOrders", None).unwrap();
    assert!(!orders
        .scripts
        .iter()
        .any(|call| call.caller == "Things/Acme.Orders.Batch.Elsewhere"));
}
