use super::compare::compare;
use super::*;
use crate::core::basetype::Mapped;
use crate::core::config::Solution;

fn definition(base_type: &str, default: &str) -> String {
    format!(
        "<ServiceDefinition name=\"GetOrder\"><ParameterDefinitions><FieldDefinition name=\"orderId\" baseType=\"{base_type}\"{default}/></ParameterDefinitions><ResultType baseType=\"STRING\"/></ServiceDefinition>"
    )
}

fn implementation(service: &str, script: &str) -> String {
    format!("<ServiceImplementation name=\"{service}\" handlerName=\"Script\"><ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row><code><![CDATA[{script}]]></code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation>")
}

const REPORT: &str = "return me.GetOrder({orderId: \"42\"});";
const CALLER: &str = "var a = Things[\"P.Orders\"].GetOrder({orderId: \"abc\"});\nvar b = Things[\"P.Orders\"].GetOrder({orderId: given});\nvar c = Things[\"P.Other\"].GetOrder({orderId: \"7\"});";

fn orders(base_type: &str, default: &str) -> String {
    format!(
        "<Entities><Things><Thing name=\"P.Orders\" projectName=\"P\"><ThingShape><ServiceDefinitions>{}<ServiceDefinition name=\"Report\"><ResultType baseType=\"STRING\"/></ServiceDefinition></ServiceDefinitions><ServiceImplementations>{}{}</ServiceImplementations></ThingShape></Thing></Things></Entities>",
        definition(base_type, default),
        implementation("GetOrder", "return orderId;"),
        implementation("Report", REPORT),
    )
}

/// `P.Orders` with sidecars and a `Report` that calls its own `GetOrder`, and `P.Caller`, kept as
/// XML only, that calls it through `Things`, with a variable, and calls another Thing's.
fn solution() -> (tempfile::TempDir, Solution) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
    std::fs::create_dir(root.join("Things")).unwrap();
    std::fs::write(root.join("Things/P.Orders.xml"), orders("STRING", "")).unwrap();
    for (service, script, definition) in [
        ("GetOrder", "return orderId;", definition("STRING", "")),
        (
            "Report",
            REPORT,
            "<ServiceDefinition name=\"Report\"><ResultType baseType=\"STRING\"/></ServiceDefinition>"
                .to_string(),
        ),
    ] {
        let folder = root.join("src/P.Orders/services").join(service);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("script.js"), script).unwrap();
        std::fs::write(folder.join("definition.xml"), format!("{definition}\n")).unwrap();
    }
    std::fs::write(
        root.join("Things/P.Caller.xml"),
        format!(
            "<Entities><Things><Thing name=\"P.Caller\" projectName=\"P\"><ThingShape><ServiceDefinitions><ServiceDefinition name=\"Run\"><ResultType baseType=\"NOTHING\"/></ServiceDefinition></ServiceDefinitions><ServiceImplementations>{}</ServiceImplementations></ThingShape></Thing></Things></Entities>",
            implementation("Run", CALLER)
        ),
    )
    .unwrap();
    let solution = Solution::load(&root.join("twaco.toml")).unwrap();
    (dir, solution)
}

fn theirs() -> Vec<Take> {
    vec![Take {
        side: TakeSide::Theirs,
        target: "P.Orders".into(),
    }]
}

#[test]
fn a_retyped_service_parameter_maps_the_literals_its_callers_pass_and_reports_the_rest() {
    let (dir, solution) = solution();
    let export = dir.path().join("export.xml");
    std::fs::write(&export, orders("LONG", " aspect.defaultValue=\"none\"")).unwrap();
    let report = compare(&solution, &export, &[]).unwrap();

    let found: Vec<_> = report
        .retyped_arguments
        .iter()
        .map(|a| {
            (
                a.caller.as_str(),
                a.literal.as_str(),
                a.mapped.clone(),
                a.from.as_str(),
                a.to.as_str(),
            )
        })
        .collect();
    // The variable is no literal, and P.Other's GetOrder is another service.
    assert_eq!(
        found,
        [
            (
                "Things/P.Caller",
                "\"abc\"",
                Mapped::Unmappable("not a whole number"),
                "STRING",
                "LONG"
            ),
            (
                "Things/P.Orders",
                "\"42\"",
                Mapped::Rewritten("42".into()),
                "STRING",
                "LONG"
            ),
        ]
    );
    let orders_sidecar = solution.root.join("src/P.Orders/services/Report/script.js");
    assert_eq!(
        report.retyped_arguments[1].file, orders_sidecar,
        "a sidecar is the source"
    );
    assert_eq!(report.retyped_arguments[0].line, 1);
    assert_eq!(report.retyped_defaults.len(), 1);
    assert_eq!(report.retyped_defaults[0].default, "none");

    let caller = std::fs::read(solution.root.join("Things/P.Caller.xml")).unwrap();
    let lock = crate::core::lock::acquire(&solution.root, "retype test", &[]).unwrap();
    apply(&solution, &export, &report, &theirs(), &lock).unwrap();
    drop(lock);
    // The literal that maps is rewritten in the sidecar and in the entity XML alike; the one that
    // does not is left as it was.
    let sidecar = std::fs::read_to_string(&orders_sidecar).unwrap();
    assert_eq!(sidecar, "return me.GetOrder({orderId: 42});");
    let xml = std::fs::read_to_string(solution.root.join("Things/P.Orders.xml")).unwrap();
    assert!(xml.contains("me.GetOrder({orderId: 42})"), "{xml}");
    assert!(xml.contains("baseType=\"LONG\""), "{xml}");
    assert_eq!(
        std::fs::read(solution.root.join("Things/P.Caller.xml")).unwrap(),
        caller
    );
}

#[test]
fn a_retype_that_is_not_taken_rewrites_no_caller() {
    let (dir, solution) = solution();
    let export = dir.path().join("export.xml");
    std::fs::write(&export, orders("LONG", "")).unwrap();
    let report = compare(&solution, &export, &[]).unwrap();
    assert_eq!(report.retyped_arguments.len(), 2);
    let before = snapshot(&solution);
    let lock = crate::core::lock::acquire(&solution.root, "retype test", &[]).unwrap();
    // With no base and no take, the entity both sides may have changed is not adopted, and so
    // neither is its retype: no caller is touched.
    apply(&solution, &export, &report, &[], &lock).unwrap();
    drop(lock);
    assert_eq!(snapshot(&solution), before);
}

fn snapshot(solution: &Solution) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    for relative in [
        "Things/P.Orders.xml",
        "Things/P.Caller.xml",
        "src/P.Orders/services/Report/script.js",
        "src/P.Orders/services/GetOrder/script.js",
    ] {
        let path = solution.root.join(relative);
        files.push((path.clone(), std::fs::read(path).unwrap()));
    }
    files
}

#[test]
fn an_export_that_retypes_nothing_reports_no_arguments() {
    let (dir, solution) = solution();
    let export = dir.path().join("export.xml");
    std::fs::write(&export, orders("STRING", "")).unwrap();
    let report = compare(&solution, &export, &[]).unwrap();
    assert!(report.retyped_arguments.is_empty());
    assert!(report.retyped_defaults.is_empty());
}
