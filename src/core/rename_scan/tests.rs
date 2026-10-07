use super::super::{refs, scan, splice};
use super::*;
use std::collections::BTreeSet;

fn apply(src: &[u8], pass: &XmlPass) -> Vec<u8> {
    splice::splice(src, &pass.edits).unwrap()
}

#[test]
fn finds_attributes_text_and_script_with_their_places() {
    let src = br#"<?xml version="1.0"?>
<!-- Acme.App.Manager -->
<Thing name="Acme.App.Manager" projectName="Acme.App.Manager" thingTemplate="Acme.App.Manager">
  <Owner>Acme.App.Manager</Owner>
  <Code><![CDATA[var x = Things["Acme.App.Manager"]; // Acme.App.Manager]]></Code>
</Thing>"#;
    let pass = scan_xml(
        src,
        "Acme.App.Manager",
        refs::Mode::Entity,
        "Acme.New.Manager",
    )
    .unwrap();
    assert_eq!(pass.findings.len(), 6);
    assert!(
        matches!(pass.findings[0].place, Place::Attribute { ref element, ref attribute } if element == "Thing" && attribute == "name")
    );
    assert!(
        matches!(pass.findings[1].place, Place::Attribute { ref attribute, .. } if attribute == "projectName")
    );
    assert!(
        matches!(pass.findings[2].place, Place::Attribute { ref attribute, .. } if attribute == "thingTemplate")
    );
    assert!(matches!(pass.findings[3].place, Place::Text { ref element } if element == "Owner"));
    assert!(pass.findings[4..]
        .iter()
        .all(|finding| matches!(finding.place, Place::Cdata { ref element } if element == "Code")));
    assert_eq!(pass.edits.len(), 6);
}

#[test]
fn entity_and_prefix_modes_treat_dotted_suffixes_differently() {
    let src = br#"<R a="Acme.App.Manager.Child" b="Acme.App.ManagerX"/>"#;
    let entity = scan_xml(
        src,
        "Acme.App.Manager",
        refs::Mode::Entity,
        "Acme.New.Manager",
    )
    .unwrap();
    assert!(entity.findings.is_empty());
    let prefix = scan_xml(src, "Acme.App", refs::Mode::Prefix, "Acme.New").unwrap();
    assert_eq!(prefix.edits.len(), 2);
}

#[test]
fn an_unqualified_name_applies_only_where_an_entity_is_named() {
    let src = br#"<Entities><Things><Thing name="T" thingTemplate="T"><PropertyDefinitions><PropertyDefinition name="T"/></PropertyDefinitions><Path>T/T1/A1</Path><Level>T</Level><Code><![CDATA[return "T"; Things["T"].Run(); Things_T;]]></Code></Thing></Things></Entities>"#;
    let pass = scan_xml(src, "T", refs::Mode::Entity, "U").unwrap();
    let tiers: Vec<(refs::Tier, bool)> = pass
        .findings
        .iter()
        .map(|finding| (finding.tier, finding.applied))
        .collect();
    assert_eq!(
        tiers,
        [
            (refs::Tier::Exact, true),   // the entity's own name
            (refs::Tier::Exact, true),   // thingTemplate names an entity
            (refs::Tier::Review, false), // a property that happens to share the word
            (refs::Tier::Review, false), // text inside a path
            (refs::Tier::Review, false), // a cell that equals the word
            (refs::Tier::Review, false), // `return "T"`: a label, not a lookup
            (refs::Tier::Exact, true),   // Things["T"]
            (refs::Tier::Exact, true),   // a mashup-derived id
        ]
    );
    assert_eq!(pass.edits.len(), 4);
}

#[test]
fn derived_ids_inside_cdata_json_are_found() {
    let src =
        br#"<Content><![CDATA[{"DynamicThingShapes_Acme.App.Management_TS": {}}]]></Content>"#;
    let pass = scan_xml(
        src,
        "Acme.App.Management_TS",
        refs::Mode::Entity,
        "Acme.New.Management_TS",
    )
    .unwrap();
    assert_eq!(pass.findings.len(), 1);
    assert_eq!(pass.findings[0].tier, refs::Tier::Embedded);
    assert_eq!(pass.edits.len(), 1);
}

#[test]
fn no_hit_has_no_edits_and_splices_identically() {
    let src = b"<R untouched=\"yes\">nothing here</R>";
    let pass = scan_xml(src, "Acme.App.X", refs::Mode::Entity, "Acme.New.X").unwrap();
    assert!(pass.findings.is_empty());
    assert!(pass.edits.is_empty());
    assert_eq!(apply(src, &pass), src);
}

#[test]
fn applied_rename_removes_old_safe_hits_and_preserves_the_count() {
    // Precondition: the replacement name does not occur in the original document.
    let src =
        br#"<R a="Acme.App.X"><T>Things_Acme.App.X</T><C><![CDATA[Things["Acme.App.X"]]]></C></R>"#;
    assert!(!String::from_utf8_lossy(src).contains("Acme.New.X"));
    let old = scan_xml(src, "Acme.App.X", refs::Mode::Entity, "Acme.New.X").unwrap();
    let renamed = apply(src, &old);
    let remaining = scan_xml(&renamed, "Acme.App.X", refs::Mode::Entity, "Acme.New.X").unwrap();
    assert!(remaining
        .findings
        .iter()
        .all(|finding| finding.tier == refs::Tier::Review));
    let new = scan_xml(&renamed, "Acme.New.X", refs::Mode::Entity, "Acme.App.X").unwrap();
    assert_eq!(new.findings.len(), old.findings.len());
}

#[test]
fn rename_round_trip_restores_attributes_text_cdata_and_derived_ids() {
    let src = br#"<R a="Acme.App.X"><T>Acme.App.X</T><C><![CDATA[Things["Acme.App.X"] = DynamicThingShapes_Acme.App.X;]]></C></R>"#;
    let forward = scan_xml(src, "Acme.App.X", refs::Mode::Entity, "Acme.New.X").unwrap();
    let renamed = apply(src, &forward);
    let reverse = scan_xml(&renamed, "Acme.New.X", refs::Mode::Entity, "Acme.App.X").unwrap();
    assert_eq!(apply(&renamed, &reverse), src);
}

#[test]
fn malformed_xml_returns_the_scanner_error() {
    let error = scan_xml(b"<R><![CDATA[unfinished", "R", refs::Mode::Entity, "S").unwrap_err();
    assert!(matches!(
        error,
        scan::ScanError::Unterminated {
            what: "CDATA section",
            ..
        }
    ));
}

#[test]
fn findings_report_lines_and_trimmed_bounded_excerpts() {
    let long = "x".repeat(140);
    let xml = format!("<R>\n  Acme.App.X  \n<C><![CDATA[{long} Acme.App.X tail]]></C>\n</R>");
    let pass = scan_xml(
        xml.as_bytes(),
        "Acme.App.X",
        refs::Mode::Entity,
        "Acme.New.X",
    )
    .unwrap();
    assert_eq!(
        pass.findings
            .iter()
            .map(|finding| finding.line)
            .collect::<Vec<_>>(),
        [2, 3]
    );
    assert_eq!(pass.findings[0].excerpt, "Acme.App.X");
    assert!(pass.findings[1].excerpt.contains("Acme.App.X"));
    assert_eq!(pass.findings[1].excerpt.chars().count(), 120);
}

#[test]
fn text_pass_uses_file_places_lines_and_line_excerpts() {
    let src = b"first line\n  const x = Things[\"Acme.App.X\"];  \nlast line";
    let pass = scan_text(src, "Acme.App.X", refs::Mode::Entity, "Acme.New.X").unwrap();
    assert_eq!(pass.findings.len(), 1);
    assert_eq!(pass.findings[0].place, Place::File);
    assert_eq!(pass.findings[0].line, 2);
    assert_eq!(
        pass.findings[0].excerpt,
        "const x = Things[\"Acme.App.X\"];"
    );
    assert_eq!(pass.edits.len(), 1);
}

#[test]
fn text_pass_renames_entity_dot_member_and_leaves_longer_entity_names_alone() {
    let qualified = Qualified {
        entities: ["Acme", "Acme.App.X", "Acme.App.X.Child"]
            .map(String::from)
            .into(),
        members: ["GetOrder", "Count"].map(String::from).into(),
    };
    let src = b"overrides = [\"Acme.App.X.GetOrder\"]\n\
        read Acme.App.X.Count.value, see Acme.App.X.Child and Acme.App.X.Child.GetOrder\n\
        Acme.App.X.Mystery here; Acme.App.X. ends a sentence\n";
    let pass = scan_text_with(
        src,
        "Acme.App.X",
        refs::Mode::Entity,
        "Acme.New.X",
        Some(&qualified),
    )
    .unwrap();
    let tiers: Vec<_> = pass
        .findings
        .iter()
        .map(|finding| (finding.line, finding.tier))
        .collect();
    assert_eq!(
        tiers,
        [
            (1, refs::Tier::Embedded),
            (2, refs::Tier::Embedded),
            // The plain pass's hit, then this pass's, on the same line.
            (3, refs::Tier::Embedded),
            (3, refs::Tier::Review),
        ]
    );
    assert_eq!(
        String::from_utf8(apply(src, &pass)).unwrap(),
        "overrides = [\"Acme.New.X.GetOrder\"]\n\
        read Acme.New.X.Count.value, see Acme.App.X.Child and Acme.App.X.Child.GetOrder\n\
        Acme.App.X.Mystery here; Acme.New.X. ends a sentence\n"
    );
    // Without what the rename knows, the qualified forms are not touched.
    let plain = scan_text(src, "Acme.App.X", refs::Mode::Entity, "Acme.New.X").unwrap();
    assert_eq!(plain.findings.len(), 1);
}

#[test]
fn an_unqualified_name_before_a_member_is_only_reported() {
    let qualified = Qualified {
        entities: ["Node"].map(String::from).into(),
        members: ["Start"].map(String::from).into(),
    };
    let src = b"Node.Start() and Node.js\n";
    let pass = scan_text_with(src, "Node", refs::Mode::Entity, "Vertex", Some(&qualified)).unwrap();
    assert_eq!(pass.findings.len(), 1);
    assert_eq!(pass.findings[0].tier, refs::Tier::Review);
    assert!(pass.edits.is_empty());
}

#[test]
fn text_pass_refuses_non_utf8_input() {
    assert!(scan_text(&[0xff], "Acme.App.X", refs::Mode::Entity, "Acme.New.X").is_err());
}

#[test]
fn an_end_tag_that_does_not_close_the_open_element_is_refused() {
    // Attributing text to the wrong element would report a place that is not there.
    for src in [
        "<A><B></A>Acme.App.X</B>",
        "<A></B>Acme.App.X</A>",
        "</A>Acme.App.X",
    ] {
        let result = scan_xml(
            src.as_bytes(),
            "Acme.App.X",
            refs::Mode::Entity,
            "Acme.New.X",
        );
        assert!(
            matches!(result, Err(scan::ScanError::Malformed { .. })),
            "{src}: {result:?}"
        );
    }
}

#[test]
fn lines_are_counted_from_one_with_crlf_and_a_hit_on_the_first_line() {
    let src = b"<A n=\"Acme.App.X\">\r\n<B>\r\n  <![CDATA[x\r\nAcme.App.X]]>\r\n</B></A>";
    let pass = scan_xml(src, "Acme.App.X", refs::Mode::Entity, "Acme.New.X").unwrap();
    let lines: Vec<usize> = pass.findings.iter().map(|finding| finding.line).collect();
    assert_eq!(lines, vec![1, 4]);
}

#[test]
fn a_hit_dense_document_is_linear_in_its_size() {
    // Thousands of hits in one large payload: a rescan of the prefix per hit would take
    // minutes here, a binary search takes milliseconds.
    let mut body = String::from("<A><![CDATA[");
    for _ in 0..40_000 {
        body.push_str("Things[\"Acme.App.X\"].Run(); // padding to make the payload large\n");
    }
    body.push_str("]]></A>");
    let started = std::time::Instant::now();
    let pass = scan_xml(
        body.as_bytes(),
        "Acme.App.X",
        refs::Mode::Entity,
        "Acme.New.X",
    )
    .unwrap();
    assert_eq!(pass.findings.len(), 40_000);
    assert_eq!(pass.findings.last().unwrap().line, 40_000);
    assert!(started.elapsed().as_secs() < 10, "{:?}", started.elapsed());
}

#[test]
fn field_pass_is_structural_exact_and_keeps_other_tables_untouched() {
    let src = br#"<Thing><ConfigurationTables>
<ConfigurationTable dataShapeName="P.D" name="T"><DataShape><FieldDefinitions><FieldDefinition name="Period"/><FieldDefinition name="PeriodDisplayName"/></FieldDefinitions></DataShape><Rows><Row><Period><![CDATA[x<y]]></Period><PeriodDisplayName>x</PeriodDisplayName></Row><Row><Period/></Row></Rows></ConfigurationTable>
<ConfigurationTable dataShapeName="P.Other" name="U"><DataShape><FieldDefinitions><FieldDefinition name="Period"/></FieldDefinitions></DataShape><Rows><Row><Period>stay</Period></Row></Rows></ConfigurationTable>
</ConfigurationTables></Thing>"#;
    let field = scan_configuration_field(src, "P.D", "Period", "PeriodKey").unwrap();
    assert_eq!(field.tables, 1);
    assert!(field.table_conflicts.is_empty());
    let changed = splice::splice(src, &field.pass.edits).unwrap();
    let expected = String::from_utf8_lossy(src)
        .replacen("name=\"Period\"", "name=\"PeriodKey\"", 1)
        .replacen(
            "<Period><![CDATA[x<y]]></Period>",
            "<PeriodKey><![CDATA[x<y]]></PeriodKey>",
            1,
        )
        .replacen("<Period/>", "<PeriodKey/>", 1);
    assert_eq!(changed, expected.as_bytes());
    assert!(String::from_utf8(changed)
        .unwrap()
        .contains("<Period>stay</Period>"));
}

#[test]
fn service_script_applies_only_resolved_callers_and_reviews_other_uses() {
    let src = br#"me . Run ();
this.Run();
Things["P.Shape"] . Run ();
Things['P.Child'].Run();
Things.Other.Run();
const a = Things["P.Shape"]; let b=Things.Other; var c = Things['P.Child'];
a.Run(); b.Run(); c.Run();
Things.Unrelated.Run(); unknown.Run(); const q = "Run"; x["Run"]; // .Run
/** @function Run */"#;
    let callers = BTreeSet::from([
        "P.Shape".to_string(),
        "P.Child".to_string(),
        "Other".to_string(),
    ]);
    let pass = scan_service_script(src, "Run", "Execute", true, true, &callers).unwrap();
    let changed = String::from_utf8(apply(src, &pass)).unwrap();
    assert_eq!(changed.matches("Execute").count(), 9);
    assert!(changed.contains("Things.Unrelated.Run()") && changed.contains("unknown.Run()"));
    assert!(
        changed.contains("\"Run\"") && changed.contains("[\"Run\"]") && changed.contains(".Run")
    );
    assert_eq!(
        pass.findings
            .iter()
            .filter(|finding| !finding.applied)
            .count(),
        5
    );
}

#[test]
fn mashup_service_pairs_are_contextual_and_leftovers_are_review() {
    let src = br#"{
  "Data": {"D": {"DataName": "D", "EntityName": "P.Shape", "Services": [{"Name": "Run", "Target": "Run"}]},
             "U": {"DataName": "U", "EntityName": "P.Other", "Services": [{"Name": "Run", "Target": "Run"}]}},
  "DataBindings": [{"SourceId": "Run", "SourceName": "Run", "SourceSection": "D", "TargetId": "Run", "TargetSection": "D"}],
  "Events": [{"EventHandlerId": "D", "EventHandlerService": "Run", "EventTriggerId": "Run", "EventTriggerSection": "D"}],
  "Label": "Run"
}"#;
    let callers = BTreeSet::from(["P.Shape".to_string()]);
    let pass = scan_service_mashup(src, "Run", "Execute", &callers, false).unwrap();
    let changed = String::from_utf8(apply(src, &pass)).unwrap();
    assert_eq!(changed.matches("Execute").count(), 7);
    assert_eq!(
        pass.findings
            .iter()
            .filter(|finding| !finding.applied)
            .count(),
        3
    );
    assert!(changed.contains("\"EntityName\": \"P.Other\", \"Services\": [{\"Name\": \"Run\""));
    assert!(changed.contains("\"Label\": \"Run\""));
}

#[test]
fn service_config_uses_spans_and_leaves_a_bare_override_for_review() {
    let src = br#"[validate]
inherited_overrides = ["P.Shape.Run", "Run", "P.Other.Run"]
[[project]]
name = "P"
[project.deploy]
entry_point_thing = "P.Child"
deploy_service = "Run"
[[project.deploy.post_import]]
thing = "P.Child"
service = "Run"
[[project.deploy.post_import]]
thing = "P.Other"
service = "Run"
"#;
    let callers = BTreeSet::from(["P.Shape".to_string(), "P.Child".to_string()]);
    let pass = scan_service_config(src, "Run", "Execute", &callers).unwrap();
    let changed = String::from_utf8(apply(src, &pass)).unwrap();
    assert!(
        changed.contains("P.Shape.Execute") && changed.contains("deploy_service = \"Execute\"")
    );
    assert!(
        changed.contains("\"Run\", \"P.Other.Run\"")
            && changed.contains("thing = \"P.Other\"\nservice = \"Run\"")
    );
    assert_eq!(
        pass.findings
            .iter()
            .filter(|finding| finding.applied)
            .count(),
        3
    );
    assert_eq!(
        pass.findings
            .iter()
            .filter(|finding| !finding.applied)
            .count(),
        1
    );
}

#[test]
fn table_pass_changes_only_real_tables_and_contextual_script_literals() {
    let src = br#"<Thing><ConfigurationTableDefinitions><ConfigurationTableDefinition name="Limits_CT" dataShapeName="P.Limits_CT"/></ConfigurationTableDefinitions><ConfigurationTables><ConfigurationTable name="Limits_CT" dataShapeName="P.Limits_CT"><DataShape/><Rows/></ConfigurationTable></ConfigurationTables><ThingShape><ServiceImplementations><ServiceImplementation name="S"><ConfigurationTables><ConfigurationTable name="Script"><Rows><Row><code><![CDATA[
let a = {tableName : "Limits_CT"}; let b = {'tableName': 'Limits_CT'}; let c = {"tableName" : "Limits_CT"};
const TABLE = "Limits_CT"; x.Limits_CT; x["Limits_CT"];
]]></code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations></ThingShape></Thing>"#;
    let scanned = scan_configuration_table(src, "Limits_CT", "Bounds_CT", true, true).unwrap();
    assert!(scanned.old_definition);
    assert_eq!(scanned.renamed_tables, 2);
    assert_eq!(scanned.new_tables, 0);
    let changed = String::from_utf8(apply(src, &scanned.pass)).unwrap();
    assert_eq!(changed.matches("name=\"Bounds_CT\"").count(), 2);
    assert!(changed.contains("name=\"Script\""));
    assert_eq!(changed.matches("tableName : \"Bounds_CT\"").count(), 1);
    assert!(
        changed.contains("'tableName': 'Bounds_CT'")
            && changed.contains("\"tableName\" : \"Bounds_CT\"")
    );
    assert!(
        changed.contains("const TABLE = \"Limits_CT\"")
            && changed.contains("x.Limits_CT")
            && changed.contains("x[\"Limits_CT\"]")
    );
    assert_eq!(
        scanned
            .pass
            .findings
            .iter()
            .filter(|finding| finding.applied)
            .count(),
        5
    );
    assert_eq!(
        scanned
            .pass
            .findings
            .iter()
            .filter(|finding| !finding.applied)
            .count(),
        3
    );

    let reviews = scan_table_script(
        br#"const x = "Limits_CT"; a.Limits_CT; a['Limits_CT'];"#,
        "Limits_CT",
        "Bounds_CT",
        false,
    )
    .unwrap();
    assert!(reviews.edits.is_empty());
    assert_eq!(reviews.findings.len(), 3);
}
#[test]
fn a_call_argument_is_not_a_shorthand_property() {
    // `f(a, cardUid, b)` has the same neighbours as `{ a, cardUid, b }`; only the bracket differs.
    let own = |script: &str| {
        scan_param_script(
            script.as_bytes(),
            "Run",
            "cardUid",
            "cardId",
            true,
            false,
            &BTreeSet::new(),
        )
        .unwrap()
    };
    let call = own("logger.warn(\"x\", me.name, cardUid, rows.length);
const n = Number(cardUid);
");
    assert_eq!(
        call.edits.len(),
        2,
        "both uses are identifiers of the parameter"
    );
    let shorthand = own("const o = { a, cardUid, b };
return Number(cardUid);
");
    assert!(
        shorthand.edits.is_empty(),
        "a shorthand property makes the whole script a review item"
    );
    assert!(shorthand.findings.iter().any(|finding| !finding.applied));
    let array = own("const list = [a, cardUid, b];
");
    assert_eq!(
        array.edits.len(),
        1,
        "an array element is an identifier use, not a property"
    );
}

#[test]
fn a_script_uses_an_identifier_only_outside_strings_comments_and_member_access() {
    let uses = |script: &str| script_uses_identifier(script.as_bytes(), "result");
    assert!(uses("var result = 1;"));
    assert!(uses("return result;"));
    assert!(
        !uses(
            "const s = \"result\"; // result
"
        ),
        "a string and a comment are not uses"
    );
    assert!(
        !uses("return row.result;"),
        "a property is not the variable"
    );
}

#[test]
fn a_service_rename_follows_run_time_permissions_keyed_by_its_name() {
    let src = br#"<Entities><ThingShapes><ThingShape name="P.S"><ServiceDefinitions><ServiceDefinition name="Run"/></ServiceDefinitions><RunTimePermissions><Permissions resourceName="Run"><ServiceInvoke><Principal name="G" type="Group"/></ServiceInvoke></Permissions><Permissions resourceName="Other"/></RunTimePermissions></ThingShape></ThingShapes></Entities>"#;
    let callers = BTreeSet::from(["P.S".to_string()]);
    let pass = scan_service_entity(src, "Run", "Execute", true, true, &callers).unwrap();
    let out = String::from_utf8(splice::splice(src, &pass.edits).unwrap()).unwrap();
    assert!(
        out.contains("resourceName=\"Execute\"") && out.contains("resourceName=\"Other\""),
        "{out}"
    );
    // An entity that is not in the callers' scope grants nothing on this service.
    let outside = scan_service_entity(src, "Run", "Execute", false, false, &callers).unwrap();
    assert!(outside.edits.is_empty());
}

#[test]
fn a_param_rename_lists_sql_placeholders_for_a_person() {
    let src = br#"<Entities><Things><Thing name="P.T"><ServiceDefinitions><ServiceDefinition name="Q"><ParameterDefinitions><FieldDefinition name="dashboardId" baseType="STRING"/></ParameterDefinitions></ServiceDefinition></ServiceDefinitions><ServiceImplementations><ServiceImplementation name="Q" handlerName="SQLQuery"><ConfigurationTables><ConfigurationTable name="Query"><Rows><Row><sql><![CDATA[SELECT * FROM t WHERE id = [[dashboardId]] AND x = <<dashboardId>>]]></sql></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations></Thing></Things></Entities>"#;
    let scanned = scan_param_entity(
        src,
        "Q",
        "dashboardId",
        "boardId",
        true,
        true,
        &BTreeSet::new(),
    )
    .unwrap();
    let review: Vec<&Finding> = scanned
        .pass
        .findings
        .iter()
        .filter(|finding| !finding.applied)
        .collect();
    assert_eq!(review.len(), 2, "{:?}", scanned.pass.findings);
    assert!(review
        .iter()
        .all(|finding| finding.excerpt.contains("SQL placeholder")));
}

/// The script with every service rename applied, for the one entity `A` and its own calls.
fn service_rename(script: &str, rename_function: bool) -> (String, XmlPass) {
    let callers = BTreeSet::from(["A".to_string()]);
    let pass = scan_service_script(
        script.as_bytes(),
        "Run",
        "Execute",
        true,
        rename_function,
        &callers,
    )
    .unwrap();
    (
        String::from_utf8(apply(script.as_bytes(), &pass)).unwrap(),
        pass,
    )
}

fn review_count(pass: &XmlPass) -> usize {
    pass.findings
        .iter()
        .filter(|finding| !finding.applied)
        .count()
}

#[test]
fn a_service_call_is_found_wherever_the_parser_finds_it() {
    // A call split over lines, and one inside a template literal, which a byte recognizer
    // read as a single opaque token.
    let (changed, _) = service_rename("Things[\"A\"]\n  .Run(1);", false);
    assert_eq!(changed, "Things[\"A\"]\n  .Execute(1);");
    let (changed, _) = service_rename("var s = `${Things.A.Run()} and ${me.Run()}`;", false);
    assert_eq!(
        changed,
        "var s = `${Things.A.Execute()} and ${me.Execute()}`;"
    );
    let (changed, _) = service_rename("var q = a / b / c; Things.A.Run();", false);
    assert_eq!(changed, "var q = a / b / c; Things.A.Execute();");
    let (changed, _) = service_rename("Things.A[\"Run\"]();", false);
    assert_eq!(changed, "Things.A[\"Execute\"]();");
}

#[test]
fn text_that_only_looks_like_a_call_is_left_for_review() {
    let script = "var s = \"me.Run()\"; var r = /me.Run()/; // me.Run()\n";
    let (changed, pass) = service_rename(script, false);
    assert_eq!(changed, script);
    assert_eq!(review_count(&pass), 3, "{:?}", pass.findings);
}

#[test]
fn a_variable_is_followed_only_when_it_can_only_be_one_caller() {
    let (changed, _) = service_rename("var t = Things.A; t.Run();", false);
    assert_eq!(changed, "var t = Things.A; t.Execute();");
    // Reassigned, redeclared as another Thing, or taken as a parameter: nothing is proved.
    for script in [
        "var t = Things.A; t = other; t.Run();",
        "var t = Things.A; var t = Things.B; t.Run();",
        "var t = Things.A; function f(t) { t.Run(); }",
        "var t = Things.A; for (t in o) { t.Run(); }",
    ] {
        let (changed, pass) = service_rename(script, false);
        assert_eq!(changed, script);
        assert!(review_count(&pass) > 0, "{script}");
    }
}

#[test]
fn a_function_tag_is_renamed_inside_comments_only() {
    let script = "var s = \"@function Run\";\n/** @function Run */\nvar t = '@function Run';";
    let (changed, _) = service_rename(script, true);
    assert_eq!(
        changed,
        "var s = \"@function Run\";\n/** @function Execute */\nvar t = '@function Run';"
    );
    let (unchanged, _) = service_rename(script, false);
    assert_eq!(unchanged, script);
}

#[test]
fn a_service_script_the_parser_refuses_is_not_edited() {
    // Rhino's `for each` is not ECMAScript; the call inside it is reported, not edited.
    let script = "for each (x in y) { Things.A.Run(); }";
    let (changed, pass) = service_rename(script, false);
    assert_eq!(changed, script);
    assert_eq!(review_count(&pass), 1, "{:?}", pass.findings);
}

fn param_rename(script: &str, own: bool, local: bool) -> (String, XmlPass) {
    let callers = BTreeSet::from(["A".to_string()]);
    let pass = scan_param_script(
        script.as_bytes(),
        "Svc",
        "old",
        "fresh",
        own,
        local,
        &callers,
    )
    .unwrap();
    (
        String::from_utf8(apply(script.as_bytes(), &pass)).unwrap(),
        pass,
    )
}

fn reasons(pass: &XmlPass) -> Vec<&str> {
    pass.findings
        .iter()
        .filter(|finding| !finding.applied)
        .map(|finding| finding.excerpt.as_str())
        .collect()
}

#[test]
fn a_free_input_is_renamed_through_templates_and_division() {
    let script = "var s = `${old}`; var q = a / old / b; f(old);";
    let (changed, _) = param_rename(script, true, false);
    assert_eq!(
        changed,
        "var s = `${fresh}`; var q = a / fresh / b; f(fresh);"
    );
    // Not a member name, an object key or a string; the key and the string are reviewed, the
    // member name is not (a dotted occurrence is never a hit).
    let script = "x.old; var o = { old: 1 }; var s = 'old';";
    let (changed, pass) = param_rename(script, true, false);
    assert_eq!(changed, script);
    assert_eq!(review_count(&pass), 2, "{:?}", pass.findings);
}

#[test]
fn a_script_that_gives_the_free_input_a_meaning_of_its_own_is_left_whole() {
    for (script, reason) in [
        ("var old = 1; use(old);", "local re-declaration"),
        ("var { a: old } = x; use(old);", "local re-declaration"),
        ("function old() {} use(old);", "local re-declaration"),
        (
            "f(function (old) {}); use(old);",
            "nested function or catch parameter",
        ),
        (
            "f((old) => old); use(old);",
            "nested function or catch parameter",
        ),
        (
            "try {} catch (old) {} use(old);",
            "nested function or catch parameter",
        ),
        ("var o = { old }; use(old);", "shorthand property"),
        ("var { old } = x; use(old);", "shorthand property"),
    ] {
        let (changed, pass) = param_rename(script, true, false);
        assert_eq!(changed, script, "{script}");
        let reasons = reasons(&pass);
        assert!(
            reasons.iter().any(|text| text.contains(reason)),
            "{script}: {reasons:?}"
        );
    }
}

#[test]
fn a_call_to_the_service_renames_only_the_keys_of_a_literal_first_argument() {
    let script = "Things.A.Svc({ old: 1, 'old': 2, other: old, [old]: 3, ...old });\n\
                      Things.B.Svc({ old: 1 });\n\
                      Things[\"A\"]\n  .Svc(\n  { old: 1 });";
    let (changed, _) = param_rename(script, false, false);
    assert_eq!(
        changed,
        "Things.A.Svc({ fresh: 1, 'fresh': 2, other: old, [old]: 3, ...old });\n\
             Things.B.Svc({ old: 1 });\n\
             Things[\"A\"]\n  .Svc(\n  { fresh: 1 });"
    );
}

#[test]
fn a_call_whose_keys_cannot_be_proved_is_reviewed() {
    for script in [
        "Things.A.Svc(args);",
        "Things.A.Svc();",
        "Things.A.Svc(...args);",
    ] {
        let (changed, pass) = param_rename(script, false, false);
        assert_eq!(changed, script);
        assert!(
            reasons(&pass)
                .iter()
                .any(|text| text.contains("non-literal first argument")),
            "{script}"
        );
    }
}

#[test]
fn a_param_call_through_me_needs_the_local_service_and_a_variable_needs_one_thing() {
    let (changed, _) = param_rename("me.Svc({ old: 1 }); this.Svc({ old: 2 });", false, true);
    assert_eq!(changed, "me.Svc({ fresh: 1 }); this.Svc({ fresh: 2 });");
    let (changed, _) = param_rename("me.Svc({ old: 1 });", false, false);
    assert_eq!(changed, "me.Svc({ old: 1 });");
    let (changed, _) = param_rename("var t = Things.A; t.Svc({ old: 1 });", false, false);
    assert_eq!(changed, "var t = Things.A; t.Svc({ fresh: 1 });");
    let ambiguous = "var t = Things.A; t = other; t.Svc({ old: 1 });";
    assert_eq!(param_rename(ambiguous, false, false).0, ambiguous);
}

#[test]
fn a_param_script_the_parser_refuses_is_left_for_review_with_the_reason() {
    let script = "for each (x in y) { old = 1; Things.A.Svc({ old: 1 }); }";
    let (changed, pass) = param_rename(script, true, false);
    assert_eq!(changed, script);
    let reasons = reasons(&pass);
    assert_eq!(reasons.len(), 2, "{reasons:?}");
    assert!(reasons
        .iter()
        .all(|text| text.contains("script could not be parsed; left for review")));
}

#[test]
fn an_identifier_use_is_found_in_a_template_and_refused_when_the_script_cannot_be_read() {
    let uses = |script: &str| script_uses_identifier(script.as_bytes(), "result");
    assert!(uses("var s = `${result}`;"));
    assert!(!uses("var s = `result`; var r = /result/; x.result;"));
    assert!(
        uses("var o = { result: 1 };"),
        "an object key counts, as before"
    );
    // Unparseable: a whole-word mention anywhere is refused, a longer word is not a mention.
    assert!(uses("for each (a in b) { // result\n }"));
    assert!(!uses("for each (a in b) { results; $result; }"));
    assert!(script_uses_identifier(
        &[b'r', b'e', b's', b'u', b'l', b't', 0xff],
        "result"
    ));
}

#[test]
fn a_table_name_is_a_selector_only_as_the_value_of_a_table_name_key() {
    let apply_table = |script: &str| {
        let pass = scan_table_script(script.as_bytes(), "Limits_CT", "Bounds_CT", true).unwrap();
        (
            String::from_utf8(apply(script.as_bytes(), &pass)).unwrap(),
            pass,
        )
    };
    let (changed, _) = apply_table("var s = `${f({ tableName: \"Limits_CT\" })}`;");
    assert_eq!(changed, "var s = `${f({ tableName: \"Bounds_CT\" })}`;");
    let (changed, _) = apply_table("f({ tableName\n  :\n  'Limits_CT' });");
    assert_eq!(changed, "f({ tableName\n  :\n  'Bounds_CT' });");
    // A conditional with the same neighbours is not an object property.
    let script = "var v = ok ? tableName : \"Limits_CT\";";
    let (changed, pass) = apply_table(script);
    assert_eq!(changed, script);
    assert_eq!(review_count(&pass), 1);
    // Another key, or a member name, is review.
    let script = "f({ other: \"Limits_CT\" }); x.Limits_CT;";
    let (changed, pass) = apply_table(script);
    assert_eq!(changed, script);
    assert_eq!(review_count(&pass), 2);
}

#[test]
fn a_table_script_the_parser_refuses_is_not_edited() {
    let script = "for each (a in b) { f({ tableName: \"Limits_CT\" }); }";
    let pass = scan_table_script(script.as_bytes(), "Limits_CT", "Bounds_CT", true).unwrap();
    assert!(pass.edits.is_empty());
    assert_eq!(review_count(&pass), 1, "{:?}", pass.findings);
}
