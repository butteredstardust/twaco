//! Writing permission blocks: layout, order, insertion, and the apply command around it.

use super::write::{rewrite, Order};
use super::{from_xml, Grant, Grants, KindKey};
use crate::core::commands::{permissions as command, Mode, Notices};
use crate::core::config::Solution;
use crate::core::entity_carry::Kind;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn group(resource: &str, name: &str) -> Grant {
    Grant {
        resource: resource.to_string(),
        action: "ServiceInvoke".to_string(),
        principal: name.to_string(),
        principal_type: "Group".to_string(),
    }
}

fn rank(name: &str) -> usize {
    ["V", "E", "A"]
        .iter()
        .position(|n| *n == name)
        .unwrap_or(usize::MAX)
}

fn by_rank() -> (impl Fn(&str) -> usize, impl Fn(&str) -> usize) {
    (rank, |name: &str| usize::from(name != "*"))
}

/// A shape as an export writes it, with CRLF line ends.
const SHAPE: &str = "<Entities>\r\n    <ThingShapes>\r\n        <ThingShape\r\n         name=\"S\">\r\n            <VisibilityPermissions>\r\n                <Visibility></Visibility>\r\n            </VisibilityPermissions>\r\n            <InstanceRunTimePermissions>\r\n                <Permissions\r\n                 resourceName=\"GetB\">\r\n                    <PropertyRead></PropertyRead>\r\n                    <PropertyWrite></PropertyWrite>\r\n                    <ServiceInvoke>\r\n                        <Principal\r\n                         isPermitted=\"true\"\r\n                         name=\"A\"\r\n                         type=\"Group\"></Principal>\r\n                    </ServiceInvoke>\r\n                    <EventInvoke></EventInvoke>\r\n                    <EventSubscribe></EventSubscribe>\r\n                </Permissions>\r\n            </InstanceRunTimePermissions>\r\n        </ThingShape>\r\n    </ThingShapes>\r\n</Entities>\r\n";

#[test]
fn a_block_already_as_wanted_is_left_byte_for_byte() {
    let current = from_xml(SHAPE.as_bytes()).unwrap();
    let (principal, resource) = by_rank();
    let order = Order {
        principal: &principal,
        resource: &resource,
    };
    let out = rewrite(SHAPE.as_bytes(), &current, &order).unwrap();
    assert_eq!(out, SHAPE.as_bytes());
}

#[test]
fn a_changed_block_keeps_its_order_adds_by_rank_and_keeps_the_layout() {
    let mut run = Grants::new();
    for grant in [
        group("GetB", "A"),
        group("GetB", "V"),
        group("GetA", "E"),
        group("GetA", "V"),
    ] {
        run.insert(grant, true);
    }
    let wanted = BTreeMap::from([(KindKey::of(Kind::InstanceRunTime), run.clone())]);
    let (principal, resource) = by_rank();
    let order = Order {
        principal: &principal,
        resource: &resource,
    };
    let out = String::from_utf8(rewrite(SHAPE.as_bytes(), &wanted, &order).unwrap()).unwrap();
    assert_eq!(
        from_xml(out.as_bytes()).unwrap()[&KindKey::of(Kind::InstanceRunTime)],
        run
    );
    // Everything outside the block is the same, CRLF included.
    assert!(out.starts_with(&SHAPE[..SHAPE.find("<InstanceRunTimePermissions>").unwrap()]));
    assert!(out.ends_with("            </InstanceRunTimePermissions>\r\n        </ThingShape>\r\n    </ThingShapes>\r\n</Entities>\r\n"));
    assert!(!out.replace("\r\n", "").contains('\n'));
    // GetB stays first with A before the new V; GetA follows, V before E by rank.
    let names: Vec<&str> = out
        .match_indices("name=\"")
        .map(|(at, _)| &out[at + 6..at + 7])
        .collect();
    assert_eq!(names, ["S", "A", "V", "V", "E"]);
    let get_b = out.find("resourceName=\"GetB\"").unwrap();
    assert!(get_b < out.find("resourceName=\"GetA\"").unwrap());
    assert!(out.contains(
        "\r\n                        <Principal\r\n                         isPermitted=\"true\"\r\n                         name=\"V\"\r\n                         type=\"Group\"></Principal>\r\n"
    ));
}

#[test]
fn an_emptied_run_time_block_closes_on_itself_and_a_missing_one_is_added() {
    let (principal, resource) = by_rank();
    let order = Order {
        principal: &principal,
        resource: &resource,
    };
    let wanted = BTreeMap::from([(KindKey::of(Kind::InstanceRunTime), Grants::new())]);
    let out = String::from_utf8(rewrite(SHAPE.as_bytes(), &wanted, &order).unwrap()).unwrap();
    assert!(
        out.contains("            <InstanceRunTimePermissions></InstanceRunTimePermissions>\r\n")
    );

    // A Thing with a visibility block but no run-time block gets one after it.
    let thing = "<Entities>\n    <Things>\n        <Thing\n         name=\"T\">\n            <VisibilityPermissions>\n                <Visibility></Visibility>\n            </VisibilityPermissions>\n            <ThingShape></ThingShape>\n        </Thing>\n    </Things>\n</Entities>\n";
    let mut run = Grants::new();
    run.insert(group("Go", "V"), true);
    let wanted = BTreeMap::from([(KindKey::of(Kind::RunTime), run.clone())]);
    let out = String::from_utf8(rewrite(thing.as_bytes(), &wanted, &order).unwrap()).unwrap();
    assert!(out.contains("            </VisibilityPermissions>\n            <RunTimePermissions>\n                <Permissions\n"), "{out}");
    assert_eq!(
        from_xml(out.as_bytes()).unwrap()[&KindKey::of(Kind::RunTime)],
        run
    );

    // With no permission block at all, before the closing tag, one step in.
    let bare = "<Entities>\n    <Things>\n        <Thing\n         name=\"T\">\n            <ThingShape></ThingShape>\n        </Thing>\n    </Things>\n</Entities>\n";
    let out = String::from_utf8(rewrite(bare.as_bytes(), &wanted, &order).unwrap()).unwrap();
    assert!(
        out.contains("<ThingShape></ThingShape>\n            <RunTimePermissions>\n"),
        "{out}"
    );
    assert!(
        out.contains("            </RunTimePermissions>\n        </Thing>\n"),
        "{out}"
    );
}

#[test]
fn a_nested_block_is_never_taken_for_the_entitys_own() {
    // A Thing's inline ThingShape can carry blocks of its own; only direct children count.
    let thing = r#"<Entities><Things><Thing name="T"><ThingShape><RunTimePermissions><Permissions resourceName="X"><ServiceInvoke><Principal isPermitted="true" name="Q" type="Group"/></ServiceInvoke></Permissions></RunTimePermissions></ThingShape><RunTimePermissions></RunTimePermissions></Thing></Things></Entities>"#;
    let mut run = Grants::new();
    run.insert(group("Go", "V"), true);
    let wanted = BTreeMap::from([(KindKey::of(Kind::RunTime), run)]);
    let (principal, resource) = by_rank();
    let order = Order {
        principal: &principal,
        resource: &resource,
    };
    let out = String::from_utf8(rewrite(thing.as_bytes(), &wanted, &order).unwrap()).unwrap();
    assert!(out.contains(r#"name="Q""#), "{out}");
}

fn temp() -> (tempfile::TempDir, PathBuf) {
    let path_guard = tempfile::Builder::new()
        .prefix("twaco-permissions-apply-")
        .tempdir()
        .unwrap();
    let path = path_guard.path().to_path_buf();
    std::fs::create_dir_all(&path).unwrap();
    (path_guard, path)
}

fn write(root: &Path, relative: &str, text: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

const POLICY: &str = r#"
strict = ["Orders_TS"]

[[role]]
name = "viewer"
group = "Viewer_UG"

[[role]]
name = "admin"
group = "Admin_UG"
includes = ["viewer"]

[[runtime]]
entities = ["Orders_TS"]
resources = ["GetOrders"]
roles = ["viewer"]

[[runtime]]
entities = ["Orders_TS"]
resources = ["DeleteOrder"]
roles = ["admin"]
"#;

const ORDERS: &str = "<Entities>\n    <ThingShapes>\n        <ThingShape\n         name=\"Acme.App.Orders_TS\"\n         projectName=\"Acme.App\">\n            <ServiceDefinitions>\n                <ServiceDefinition\n                 name=\"GetOrders\"></ServiceDefinition>\n                <ServiceDefinition\n                 name=\"DeleteOrder\"></ServiceDefinition>\n            </ServiceDefinitions>\n            <VisibilityPermissions>\n                <Visibility></Visibility>\n            </VisibilityPermissions>\n            <InstanceRunTimePermissions></InstanceRunTimePermissions>\n        </ThingShape>\n    </ThingShapes>\n</Entities>\n";

fn solution(policy: &str) -> (tempfile::TempDir, Solution, PathBuf) {
    let (_dir, root) = temp();
    write(&root, "twaco.toml", "[[project]]\nname = \"Acme.App\"\n");
    write(&root, "permissions.toml", policy);
    write(&root, "ThingShapes/Acme.App.Orders_TS.xml", ORDERS);
    (
        _dir,
        Solution::load(&root.join("twaco.toml")).unwrap(),
        root,
    )
}

fn request(mode: Mode) -> command::ApplyRequest {
    command::ApplyRequest {
        project: None,
        mode,
        lock_label: "permissions apply",
    }
}

#[test]
fn apply_plans_writes_once_and_then_has_nothing_to_do() {
    let (_dir, solution, root) = solution(POLICY);
    let path = root.join("ThingShapes/Acme.App.Orders_TS.xml");
    let plan =
        command::execute_apply(&solution, &request(Mode::Plan), &mut Notices::default()).unwrap();
    assert!(!plan.applied);
    let changes: Vec<_> = plan.plan.changes().collect();
    assert_eq!(changes.len(), 1);
    // Viewer and admin on GetOrders, admin on DeleteOrder; visibility to both roles' units.
    assert_eq!((changes[0].added, changes[0].removed), (5, 0));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        ORDERS,
        "a plan writes nothing"
    );

    let applied =
        command::execute_apply(&solution, &request(Mode::Apply), &mut Notices::default()).unwrap();
    assert!(applied.applied);
    let written = std::fs::read_to_string(&path).unwrap();
    assert_ne!(written, ORDERS);
    let sets = from_xml(written.as_bytes()).unwrap();
    assert_eq!(sets[&KindKey::of(Kind::InstanceRunTime)].len(), 3);
    assert_eq!(sets[&KindKey::of(Kind::Visibility)].len(), 2);
    assert!(!root
        .join(".twaco/transactions")
        .read_dir()
        .is_ok_and(|mut d| d.next().is_some()));

    let again =
        command::execute_apply(&solution, &request(Mode::Plan), &mut Notices::default()).unwrap();
    assert_eq!(again.plan.changes().count(), 0);
    let audit = super::audit::audit(&solution, None).unwrap();
    assert_eq!(audit.count(super::audit::Severity::Error), 0, "{audit:#?}");
}

#[test]
fn apply_refuses_while_a_strict_service_is_unclassified() {
    let policy = POLICY.replace("resources = [\"DeleteOrder\"]", "resources = [\"Remove*\"]");
    let (_dir, solution, root) = solution(&policy);
    let error = command::execute_apply(&solution, &request(Mode::Apply), &mut Notices::default())
        .unwrap_err()
        .to_string();
    assert!(error.contains("DeleteOrder"), "{error}");
    assert_eq!(
        std::fs::read_to_string(root.join("ThingShapes/Acme.App.Orders_TS.xml")).unwrap(),
        ORDERS
    );
}

#[test]
fn two_missing_blocks_go_in_together() {
    let (principal, resource) = by_rank();
    let order = Order {
        principal: &principal,
        resource: &resource,
    };
    let mut run = Grants::new();
    run.insert(group("Go", "V"), true);
    let mut visible = Grants::new();
    visible.insert(
        Grant {
            resource: String::new(),
            action: "Visibility".to_string(),
            principal: "O:V".to_string(),
            principal_type: "OrganizationalUnit".to_string(),
        },
        true,
    );
    let wanted = BTreeMap::from([
        (KindKey::of(Kind::RunTime), run.clone()),
        (KindKey::of(Kind::Visibility), visible.clone()),
    ]);
    // No permission block at all: both go before the closing tag.
    let bare = "<Entities>\n    <Things>\n        <Thing\n         name=\"T\">\n            <ThingShape></ThingShape>\n        </Thing>\n    </Things>\n</Entities>\n";
    let out = String::from_utf8(rewrite(bare.as_bytes(), &wanted, &order).unwrap()).unwrap();
    let sets = from_xml(out.as_bytes()).unwrap();
    assert_eq!(sets[&KindKey::of(Kind::RunTime)], run);
    assert_eq!(sets[&KindKey::of(Kind::Visibility)], visible);
    assert!(
        out.contains("            </RunTimePermissions>\n            <VisibilityPermissions>\n"),
        "{out}"
    );
    assert!(
        out.contains("            </VisibilityPermissions>\n        </Thing>\n"),
        "{out}"
    );

    // A replaced visibility block, with the missing run-time block after it.
    let with_visibility = "<Entities>\n    <Things>\n        <Thing\n         name=\"T\">\n            <VisibilityPermissions>\n                <Visibility></Visibility>\n            </VisibilityPermissions>\n        </Thing>\n    </Things>\n</Entities>\n";
    let out =
        String::from_utf8(rewrite(with_visibility.as_bytes(), &wanted, &order).unwrap()).unwrap();
    let sets = from_xml(out.as_bytes()).unwrap();
    assert_eq!(sets[&KindKey::of(Kind::RunTime)], run);
    assert_eq!(sets[&KindKey::of(Kind::Visibility)], visible);
    assert!(
        out.contains("            </VisibilityPermissions>\n            <RunTimePermissions>\n"),
        "{out}"
    );
}

#[test]
fn a_file_changed_after_the_plan_is_refused_not_overwritten() {
    let (_dir, solution, root) = solution(POLICY);
    let path = root.join("ThingShapes/Acme.App.Orders_TS.xml");
    let plan = super::apply::plan(&solution, None).unwrap();
    let change = plan.changes().next().unwrap();
    // Someone edits the file after it was read for the plan.
    let edited = ORDERS.replace("<Visibility></Visibility>", "<Visibility><Principal isPermitted=\"true\" name=\"Keep\" type=\"Organization\"/></Visibility>");
    std::fs::write(&path, &edited).unwrap();
    let lock = crate::core::lock::acquire_for(&solution, "test").unwrap();
    let mut operation = crate::core::transaction::Transaction::new(&solution.root, "test");
    operation
        .replace_file(&change.path, &change.before, change.after.clone())
        .unwrap();
    assert!(operation.apply(&lock).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), edited);
    drop(lock);
}

#[test]
fn what_the_write_settles_is_not_left_over_and_what_it_cannot_fails_the_run() {
    // An Organization granted run time on a managed shape: apply removes it, so nothing remains.
    let bad = ORDERS.replace(
        "<InstanceRunTimePermissions></InstanceRunTimePermissions>",
        "<InstanceRunTimePermissions><Permissions resourceName=\"GetOrders\"><ServiceInvoke><Principal isPermitted=\"true\" name=\"Acme.App.Default_OR\" type=\"Organization\"/></ServiceInvoke></Permissions></InstanceRunTimePermissions>",
    );
    let (_dir, solution, root) = solution(POLICY);
    write(&root, "ThingShapes/Acme.App.Orders_TS.xml", &bad);
    let plan = super::apply::plan(&solution, None).unwrap();
    assert_eq!(
        plan.remaining_errors(),
        0,
        "{:#?}",
        plan.projects[0].remaining
    );

    // A group in a visibility block is not the policy's to drop: it remains, as an error.
    let group = ORDERS.replace(
        "<Visibility></Visibility>",
        "<Visibility><Principal isPermitted=\"true\" name=\"Acme.App.Viewer_UG\" type=\"Group\"/></Visibility>",
    );
    write(&root, "ThingShapes/Acme.App.Orders_TS.xml", &group);
    let plan = super::apply::plan(&solution, None).unwrap();
    assert_eq!(
        plan.remaining_errors(),
        1,
        "{:#?}",
        plan.projects[0].remaining
    );
}

fn empty_table(name: &str, data_shape: &str) -> String {
    format!("                <ConfigurationTable\n                 dataShapeName=\"{data_shape}\"\n                 description=\"\"\n                 isMultiRow=\"true\"\n                 name=\"{name}\"\n                 ordinal=\"0\">\n                    <DataShape>\n                        <FieldDefinitions></FieldDefinitions>\n                    </DataShape>\n                    <Rows></Rows>\n                </ConfigurationTable>\n")
}

fn helper_solution() -> (tempfile::TempDir, Solution, PathBuf) {
    let (_dir, solution, root) = solution(POLICY);
    let tables = [
        empty_table("RoleGroupsAndOrganizations", ""),
        empty_table("RunTimePermissionsTable", "Acme.App.RunTimePermissions_DS"),
        empty_table(
            "VisibilityPermissionsTable",
            "Acme.App.VisibilityPermissions_DS",
        ),
    ]
    .concat();
    write(
        &root,
        "Things/Acme.App.ComponentPermissionHelper.xml",
        &format!("<Entities>\n    <Things>\n        <Thing\n         name=\"Acme.App.ComponentPermissionHelper\"\n         projectName=\"Acme.App\"\n         thingTemplate=\"PTCDTS.Base.ComponentPermissionHelper_TT\">\n            <VisibilityPermissions>\n                <Visibility></Visibility>\n            </VisibilityPermissions>\n            <RunTimePermissions></RunTimePermissions>\n            <ConfigurationTables>\n{tables}            </ConfigurationTables>\n        </Thing>\n    </Things>\n</Entities>\n"),
    );
    for shape in ["RunTimePermissions_DS", "VisibilityPermissions_DS"] {
        write(
            &root,
            &format!("DataShapes/Acme.App.{shape}.xml"),
            &format!("<Entities>\n    <DataShapes>\n        <DataShape\n         name=\"Acme.App.{shape}\"\n         projectName=\"Acme.App\">\n            <VisibilityPermissions>\n                <Visibility></Visibility>\n            </VisibilityPermissions>\n            <FieldDefinitions></FieldDefinitions>\n        </DataShape>\n    </DataShapes>\n</Entities>\n"),
        );
    }
    (_dir, solution, root)
}

#[test]
fn helper_mode_writes_the_helpers_tables_and_columns_from_the_policy() {
    let (_dir, solution, root) = helper_solution();
    let audit = super::audit::audit(&solution, None).unwrap();
    assert_eq!(audit.projects[0].mode, "helper");
    let codes: Vec<&str> = audit.projects[0].findings.iter().map(|f| f.code).collect();
    assert!(codes.contains(&"helper-differs-from-policy"), "{codes:?}");

    command::execute_apply(&solution, &request(Mode::Apply), &mut Notices::default()).unwrap();
    let helper = std::fs::read(root.join("Things/Acme.App.ComponentPermissionHelper.xml")).unwrap();
    let tables = super::helper::read_tables(&crate::core::normalise::entity_of(&helper).unwrap());
    let roles: Vec<(&str, &str)> = tables["RoleGroupsAndOrganizations"]
        .rows
        .iter()
        .map(|r| (r["displayName"].as_str(), r["principal"].as_str()))
        .collect();
    assert_eq!(
        roles,
        [
            ("adminGroup", "Acme.App.Admin_UG"),
            ("adminOrg", "Acme.App.Default_OR:Acme.App.Admin_UG"),
            ("viewerGroup", "Acme.App.Viewer_UG"),
            ("viewerOrg", "Acme.App.Default_OR:Acme.App.Viewer_UG"),
        ]
    );
    // A row per service, with the role columns of the policy; the admin includes the viewer.
    let run_time = &tables["RunTimePermissionsTable"];
    let row = |service: &str| {
        run_time
            .rows
            .iter()
            .find(|r| r["resource"] == service)
            .unwrap()
            .clone()
    };
    assert_eq!(row("GetOrders")["viewerGroup"], "true");
    assert_eq!(row("GetOrders")["adminGroup"], "true");
    assert_eq!(row("DeleteOrder")["viewerGroup"], "false");
    assert_eq!(row("DeleteOrder")["ID"], "1.0", "services in name order");
    let visibility = &tables["VisibilityPermissionsTable"];
    assert_eq!(visibility.rows.len(), 4, "every entity of the project");
    assert!(visibility.rows.iter().all(|r| r["viewerOrg"] == "true"));
    let shape = std::fs::read_to_string(root.join("DataShapes/Acme.App.RunTimePermissions_DS.xml"))
        .unwrap();
    assert!(
        shape.contains("name=\"viewerGroup\"\n                 ordinal=\"6\"></FieldDefinition>"),
        "{shape}"
    );

    // Written once, then nothing differs; the audit agrees.
    let again =
        command::execute_apply(&solution, &request(Mode::Plan), &mut Notices::default()).unwrap();
    assert_eq!(again.plan.changes().count(), 0);
    let audit = super::audit::audit(&solution, None).unwrap();
    assert_eq!(audit.count(super::audit::Severity::Error), 0, "{audit:#?}");

    // A new role is a new column in both tables and both DataShapes, and keeps the rows' IDs.
    let policy = std::fs::read_to_string(root.join("permissions.toml")).unwrap()
        + "\n[[role]]\nname = \"auditor\"\ngroup = \"Auditor_UG\"\n";
    std::fs::write(root.join("permissions.toml"), policy).unwrap();
    let plan =
        command::execute_apply(&solution, &request(Mode::Apply), &mut Notices::default()).unwrap();
    let changed: Vec<&str> = plan.plan.changes().map(|c| c.entity.as_str()).collect();
    assert!(
        changed.contains(&"DataShapes/Acme.App.RunTimePermissions_DS"),
        "{changed:?}"
    );
    assert!(
        changed.contains(&"Things/Acme.App.ComponentPermissionHelper"),
        "{changed:?}"
    );
    let helper = std::fs::read(root.join("Things/Acme.App.ComponentPermissionHelper.xml")).unwrap();
    let tables = super::helper::read_tables(&crate::core::normalise::entity_of(&helper).unwrap());
    let run_time = &tables["RunTimePermissionsTable"];
    assert!(run_time.rows.iter().all(|r| r["auditorGroup"] == "false"));
    assert_eq!(
        run_time
            .rows
            .iter()
            .find(|r| r["resource"] == "DeleteOrder")
            .unwrap()["ID"],
        "1.0"
    );
}

#[test]
fn a_helper_that_cannot_mean_one_thing_is_an_error_not_a_guess() {
    // Two run-time tables, the last one correct: the first must not hide behind it.
    let (_dir, solution, root) = helper_solution();
    command::execute_apply(&solution, &request(Mode::Apply), &mut Notices::default()).unwrap();
    let path = root.join("Things/Acme.App.ComponentPermissionHelper.xml");
    let written = std::fs::read_to_string(&path).unwrap();
    let doubled = written.replacen(
        "            <ConfigurationTables>\n",
        &format!(
            "            <ConfigurationTables>\n{}",
            empty_table("RunTimePermissionsTable", "Acme.App.RunTimePermissions_DS")
        ),
        1,
    );
    std::fs::write(&path, &doubled).unwrap();
    let audit = super::audit::audit(&solution, None).unwrap();
    let codes: Vec<&str> = audit.projects[0].findings.iter().map(|f| f.code).collect();
    assert!(codes.contains(&"helper-unwritable"), "{codes:?}");
    assert!(
        command::execute_apply(&solution, &request(Mode::Plan), &mut Notices::default()).is_err()
    );
    std::fs::write(&path, &written).unwrap();

    // A table pointing at another DataShape is pointed back at the project's.
    let foreign = written.replace(
        "dataShapeName=\"Acme.App.RunTimePermissions_DS\"",
        "dataShapeName=\"Other.RunTimePermissions_DS\"",
    );
    std::fs::write(&path, &foreign).unwrap();
    command::execute_apply(&solution, &request(Mode::Apply), &mut Notices::default()).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), written);

    // Without the project's DataShape, the helper cannot be kept: an error.
    std::fs::remove_file(root.join("DataShapes/Acme.App.VisibilityPermissions_DS.xml")).unwrap();
    let audit = super::audit::audit(&solution, None).unwrap();
    let finding = audit.projects[0]
        .findings
        .iter()
        .find(|f| f.code == "helper-unwritable")
        .unwrap();
    assert!(
        finding
            .message
            .contains("Acme.App.VisibilityPermissions_DS"),
        "{finding:?}"
    );
}

#[test]
fn the_servers_helper_tables_are_compared_as_sets_of_rows() {
    use super::server_audit_tests::Fake;
    let (_dir, solution, root) = helper_solution();
    command::execute_apply(&solution, &request(Mode::Apply), &mut Notices::default()).unwrap();
    let helper = std::fs::read(root.join("Things/Acme.App.ComponentPermissionHelper.xml")).unwrap();
    let tables = super::helper::read_tables(&crate::core::normalise::entity_of(&helper).unwrap());
    // As the server returns them: in another order, with JSON booleans and numbers.
    let infotable = |name: &str| {
        let table = &tables[name];
        let mut rows: Vec<serde_json::Value> = table
            .rows
            .iter()
            .map(|row| {
                let mut object = serde_json::Map::new();
                for (field, value) in row {
                    let json = match value.as_str() {
                        "true" => serde_json::json!(true),
                        "false" => serde_json::json!(false),
                        number if field == "ID" => {
                            serde_json::json!(number.parse::<f64>().unwrap())
                        }
                        text => serde_json::json!(text),
                    };
                    object.insert(field.clone(), json);
                }
                serde_json::Value::Object(object)
            })
            .collect();
        rows.reverse();
        let fields: serde_json::Map<String, serde_json::Value> = table
            .fields
            .iter()
            .map(|f| (f.name.clone(), serde_json::json!({"name": f.name})))
            .collect();
        serde_json::json!({"dataShape": {"fieldDefinitions": fields}, "rows": rows})
    };
    let mut fake = Fake::default();
    for name in [
        "RoleGroupsAndOrganizations",
        "RunTimePermissionsTable",
        "VisibilityPermissionsTable",
    ] {
        fake.tables.insert(name.to_string(), infotable(name));
    }
    let report = super::audit::audit_with(&solution, None, Some(&fake)).unwrap();
    let helper_findings: Vec<_> = report.projects[0]
        .findings
        .iter()
        .filter(|f| f.code.starts_with("server-helper") || f.code == "server-unreadable")
        .collect();
    assert!(helper_findings.is_empty(), "{helper_findings:#?}");

    // One cell differs on the server.
    let mut changed = infotable("RunTimePermissionsTable");
    changed["rows"][0]["viewerGroup"] = serde_json::json!(true);
    changed["rows"][0]["adminGroup"] = serde_json::json!(false);
    fake.tables
        .insert("RunTimePermissionsTable".to_string(), changed);
    let report = super::audit::audit_with(&solution, None, Some(&fake)).unwrap();
    assert!(report.projects[0]
        .findings
        .iter()
        .any(|f| f.code == "server-helper-differs"));

    // A helper the server does not have yet is the entity audit's to report, once.
    fake.missing
        .push("Acme.App.ComponentPermissionHelper".to_string());
    let report = super::audit::audit_with(&solution, None, Some(&fake)).unwrap();
    let codes: Vec<&str> = report.projects[0].findings.iter().map(|f| f.code).collect();
    assert!(!codes.contains(&"server-helper-differs"), "{codes:?}");
    assert!(!codes.contains(&"server-unreadable"), "{codes:?}");
}

#[test]
fn a_drafted_policy_changes_nothing_and_leaves_what_it_cannot_say_alone() {
    use crate::core::commands::permissions::InitRequest;
    let init = |from_helper: bool| InitRequest {
        project: None,
        from_helper,
        mode: Mode::Apply,
        lock_label: "permissions init",
    };
    // A project as the policy left it, drafted from its XML and from its helper.
    for from_helper in [false, true] {
        let (_dir, solution, root) = helper_solution();
        command::execute_apply(&solution, &request(Mode::Apply), &mut Notices::default()).unwrap();
        std::fs::remove_file(root.join("permissions.toml")).unwrap();
        let drafted =
            command::execute_init(&solution, &init(from_helper), &mut Notices::default()).unwrap();
        assert!(drafted.written);
        assert!(
            drafted.drafts[0].notes.is_empty(),
            "{:?}",
            drafted.drafts[0].notes
        );
        let plan = command::execute_apply(&solution, &request(Mode::Plan), &mut Notices::default())
            .unwrap();
        assert_eq!(plan.plan.changes().count(), 0, "{}", drafted.drafts[0].text);
        // An existing policy is never overwritten.
        assert!(
            command::execute_init(&solution, &init(from_helper), &mut Notices::default()).is_err()
        );
    }

    // A deny cannot be said: the entity is left unmanaged, with a note.
    let (_dir, solution, root) = solution(POLICY);
    write(
        &root,
        "ThingShapes/Acme.App.Orders_TS.xml",
        &ORDERS.replace(
            "<InstanceRunTimePermissions></InstanceRunTimePermissions>",
            "<InstanceRunTimePermissions><Permissions resourceName=\"GetOrders\"><ServiceInvoke><Principal isPermitted=\"false\" name=\"Acme.App.Viewer_UG\" type=\"Group\"/></ServiceInvoke></Permissions></InstanceRunTimePermissions>",
        ),
    );
    std::fs::remove_file(root.join("permissions.toml")).unwrap();
    let drafted = command::execute_init(&solution, &init(false), &mut Notices::default()).unwrap();
    assert!(
        drafted.drafts[0].notes[0].contains("unmanaged"),
        "{:?}",
        drafted.drafts[0].notes
    );
    assert!(drafted.drafts[0]
        .text
        .contains("unmanaged = [\"Orders_TS\"]"));
    let plan =
        command::execute_apply(&solution, &request(Mode::Plan), &mut Notices::default()).unwrap();
    assert_eq!(plan.plan.changes().count(), 0);
}

#[test]
fn a_helper_that_cannot_be_kept_by_its_own_draft_is_refused() {
    use crate::core::commands::permissions::InitRequest;
    let init = |from_helper: bool| InitRequest {
        project: None,
        from_helper,
        mode: Mode::Plan,
        lock_label: "permissions init",
    };
    let (_dir, solution, root) = helper_solution();
    command::execute_apply(&solution, &request(Mode::Apply), &mut Notices::default()).unwrap();
    std::fs::remove_file(root.join("permissions.toml")).unwrap();
    let path = root.join("Things/Acme.App.ComponentPermissionHelper.xml");
    let written = std::fs::read_to_string(&path).unwrap();

    // The visibility table lost the shape's row.
    let start = written.rfind("Acme.App.Orders_TS\n").unwrap();
    let row_start = written[..start].rfind("<Row>").unwrap();
    let row_end = written[start..].find("</Row>").unwrap() + start + "</Row>".len();
    let visibility = written.find("name=\"VisibilityPermissionsTable\"").unwrap();
    assert!(
        row_start > visibility,
        "the last Orders_TS row is the visibility one"
    );
    std::fs::write(
        &path,
        format!("{}{}", &written[..row_start], &written[row_end..]),
    )
    .unwrap();
    let error = command::execute_init(&solution, &init(true), &mut Notices::default())
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("VisibilityPermissionsTable: add row"),
        "{error}"
    );
    // From the entity XML it drafts, and says the helper's tables will be rewritten.
    let drafted = command::execute_init(&solution, &init(false), &mut Notices::default()).unwrap();
    assert!(drafted.drafts[0]
        .notes
        .iter()
        .any(|n| n.contains("rewrite the permission helper")));

    // A role mapped to a group without a dot cannot be said in a policy.
    let dotless = written.replacen(
        "                            Acme.App.Viewer_UG\n",
        "                            Shared_UG\n",
        1,
    );
    assert_ne!(dotless, written);
    std::fs::write(&path, dotless).unwrap();
    let error = command::execute_init(&solution, &init(false), &mut Notices::default())
        .unwrap_err()
        .to_string();
    assert!(error.contains("Shared_UG"), "{error}");
}

#[test]
fn a_short_name_that_would_name_two_entities_is_written_whole() {
    let (_dir, solution, root) = solution(POLICY);
    command::execute_apply(&solution, &request(Mode::Apply), &mut Notices::default()).unwrap();
    // An entity literally called `Orders_TS`, beside `Acme.App.Orders_TS`, granting nothing.
    write(
        &root,
        "ThingShapes/Orders_TS.xml",
        "<Entities><ThingShapes><ThingShape name=\"Orders_TS\" projectName=\"Acme.App\"><InstanceRunTimePermissions></InstanceRunTimePermissions></ThingShape></ThingShapes></Entities>",
    );
    std::fs::remove_file(root.join("permissions.toml")).unwrap();
    let drafted = command::execute_init(
        &solution,
        &crate::core::commands::permissions::InitRequest {
            project: None,
            from_helper: false,
            mode: Mode::Apply,
            lock_label: "permissions init",
        },
        &mut Notices::default(),
    )
    .unwrap();
    assert!(
        drafted.drafts[0].text.contains("\"Acme.App.Orders_TS\""),
        "{}",
        drafted.drafts[0].text
    );
    let plan =
        command::execute_apply(&solution, &request(Mode::Plan), &mut Notices::default()).unwrap();
    assert_eq!(plan.plan.changes().count(), 0, "{}", drafted.drafts[0].text);
}

#[test]
fn a_visibility_deny_of_a_roles_unit_is_left_alone_by_the_draft() {
    let (_dir, solution, root) = solution(POLICY);
    command::execute_apply(&solution, &request(Mode::Apply), &mut Notices::default()).unwrap();
    let path = root.join("ThingShapes/Acme.App.Orders_TS.xml");
    let written = std::fs::read_to_string(&path).unwrap();
    // The viewer's unit is denied, not allowed.
    let marker = "name=\"Acme.App.Default_OR:Acme.App.Viewer_UG\"";
    let at = written.find(marker).unwrap();
    let permitted = written[..at].rfind("isPermitted=\"true\"").unwrap();
    let denied = format!(
        "{}isPermitted=\"false\"{}",
        &written[..permitted],
        &written[permitted + "isPermitted=\"true\"".len()..]
    );
    std::fs::write(&path, &denied).unwrap();
    // The roles' units exist, so a drafted role owns its unit.
    write(
        &root,
        "Organizations/Acme.App.Default_OR.xml",
        "<Entities><Organizations><Organization name=\"Acme.App.Default_OR\" projectName=\"Acme.App\"><OrganizationalUnits><OrganizationalUnit name=\"Acme.App.Viewer_UG\"/><OrganizationalUnit name=\"Acme.App.Admin_UG\"/></OrganizationalUnits></Organization></Organizations></Entities>",
    );
    std::fs::remove_file(root.join("permissions.toml")).unwrap();
    let drafted = command::execute_init(
        &solution,
        &crate::core::commands::permissions::InitRequest {
            project: None,
            from_helper: false,
            mode: Mode::Apply,
            lock_label: "permissions init",
        },
        &mut Notices::default(),
    )
    .unwrap();
    assert!(
        drafted.drafts[0].notes.iter().any(|n| n.contains("denies")),
        "{:?}",
        drafted.drafts[0].notes
    );
    let plan =
        command::execute_apply(&solution, &request(Mode::Plan), &mut Notices::default()).unwrap();
    assert_eq!(plan.plan.changes().count(), 0, "{}", drafted.drafts[0].text);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), denied);
}
