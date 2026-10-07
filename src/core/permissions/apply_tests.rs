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

fn temp() -> PathBuf {
    let nonce = crate::test_nonce();
    let path = std::env::temp_dir().join(format!(
        "twaco-permissions-apply-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
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

fn solution(policy: &str) -> (Solution, PathBuf) {
    let root = temp();
    write(&root, "twaco.toml", "[[project]]\nname = \"Acme.App\"\n");
    write(&root, "permissions.toml", policy);
    write(&root, "ThingShapes/Acme.App.Orders_TS.xml", ORDERS);
    (Solution::load(&root.join("twaco.toml")).unwrap(), root)
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
    let (solution, root) = solution(POLICY);
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
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn apply_refuses_while_a_strict_service_is_unclassified() {
    let policy = POLICY.replace("resources = [\"DeleteOrder\"]", "resources = [\"Remove*\"]");
    let (solution, root) = solution(&policy);
    let error = command::execute_apply(&solution, &request(Mode::Apply), &mut Notices::default())
        .unwrap_err()
        .to_string();
    assert!(error.contains("DeleteOrder"), "{error}");
    assert_eq!(
        std::fs::read_to_string(root.join("ThingShapes/Acme.App.Orders_TS.xml")).unwrap(),
        ORDERS
    );
    let _ = std::fs::remove_dir_all(root);
}
