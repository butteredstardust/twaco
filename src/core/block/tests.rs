use super::*;
use crate::core::newblock;
use std::cell::RefCell;

/// A solution with one block made as `new building-block` makes it, and a project that is not one.
fn solution() -> (tempfile::TempDir, Solution) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(
        root.join("twaco.toml"),
        "[[project]]\nname = \"Acme.Plain\"\nroot = \"plain\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("plain/Things")).unwrap();
    std::fs::write(
        root.join("plain/Things/Acme.Plain.EntryPoint.xml"),
        "<Entities><Things><Thing name=\"Acme.Plain.EntryPoint\" projectName=\"Acme.Plain\"><ThingProperties></ThingProperties></Thing></Things></Entities>",
    )
    .unwrap();
    let solution = Solution::load(&root.join("twaco.toml")).unwrap();
    let request = newblock::Request {
        name: "Acme.Orders".into(),
        kind: newblock::BlockType::Standard,
        display_name: Some("Orders".into()),
        description: "Order handling".into(),
        parent: None,
        model_logic: false,
        management_shape: true,
        root: None,
        base_extension: Some("PTC.Base:9.7.0".into()),
    };
    let plan = newblock::plan(&solution, &request).unwrap();
    let lock = crate::core::lock::acquire(root, "test", &[]).unwrap();
    newblock::apply(&solution, &plan, &lock).unwrap();
    drop(lock);
    let solution = Solution::load(&root.join("twaco.toml")).unwrap();
    (dir, solution)
}

#[test]
fn a_block_is_read_from_its_entry_point_and_project() {
    let (_dir, solution) = solution();
    let all = blocks(&solution);
    assert_eq!(
        all.len(),
        1,
        "a Thing named *.EntryPoint without componentType is no block"
    );
    let block = &all[0];
    assert_eq!(block.name, "Acme.Orders");
    assert_eq!(block.kind, "Standard");
    assert_eq!(block.version, "1.0.0");
    assert_eq!(block.project_version.as_deref(), Some("1.0.0"));
    assert_eq!(block.display_name, "Orders");
    assert_eq!(block.description, "Order handling");
    assert_eq!(block.entry_point, "Acme.Orders.EntryPoint");
    assert_eq!(block.entry_point_template, "Acme.Orders.EntryPoint_TT");
    assert_eq!(block.manager.as_deref(), Some("Acme.Orders.Manager"));
    assert_eq!(block.default_organization, "Acme.Orders.Default_OR");
    assert_eq!(block.default_group, "Acme.Orders.Default_UG");
    assert_eq!(block.permission_helper, None);
    assert_eq!(block.file, "Acme.Orders/Things/Acme.Orders.EntryPoint.xml");

    assert_eq!(find(&solution, "Orders").unwrap().name, "Acme.Orders");
    let error = find(&solution, "Other").unwrap_err();
    assert_eq!(
        error,
        "no building block is named Other; the solution's are Acme.Orders"
    );
}

#[test]
fn a_version_change_rewrites_the_two_values_and_nothing_else() {
    let (_dir, solution) = solution();
    let plan = plan_version(&solution, "Acme.Orders", &VersionChange::Bump(Bump::Minor)).unwrap();
    assert_eq!((plan.from.as_str(), plan.to.as_str()), ("1.0.0", "1.1.0"));
    assert_eq!(
        plan.files,
        [
            "Acme.Orders/Projects/Acme.Orders.xml",
            "Acme.Orders/Things/Acme.Orders.EntryPoint.xml"
        ]
    );
    for (_, before, after) in &plan.writes {
        let before = String::from_utf8_lossy(before);
        let after = String::from_utf8_lossy(after);
        assert_eq!(before.replacen("1.0.0", "1.1.0", 1), after);
    }
    let lock = crate::core::lock::acquire(&solution.root, "test", &[]).unwrap();
    apply_version(&solution, &plan, &lock).unwrap();
    drop(lock);
    let block = find(&solution, "Acme.Orders").unwrap();
    assert_eq!(block.version, "1.1.0");
    assert_eq!(block.project_version.as_deref(), Some("1.1.0"));

    let error = plan_version(
        &solution,
        "Acme.Orders",
        &VersionChange::Set("1.1.0".into()),
    )
    .unwrap_err();
    assert_eq!(error, "Acme.Orders is at 1.1.0 already");
    let error =
        plan_version(&solution, "Acme.Orders", &VersionChange::Set("1.2".into())).unwrap_err();
    assert!(error.contains("not a version"), "{error}");
}

#[test]
fn versions_that_disagree_are_bumped_only_by_setting_both() {
    let (_dir, solution) = solution();
    let project = solution.root.join("Acme.Orders/Projects/Acme.Orders.xml");
    let text = std::fs::read_to_string(&project).unwrap();
    std::fs::write(
        &project,
        text.replace("packageVersion=\"1.0.0\"", "packageVersion=\"2.0.0\""),
    )
    .unwrap();
    let error =
        plan_version(&solution, "Acme.Orders", &VersionChange::Bump(Bump::Patch)).unwrap_err();
    assert!(error.contains("make them agree first"), "{error}");
    let plan = plan_version(
        &solution,
        "Acme.Orders",
        &VersionChange::Set("2.0.1".into()),
    )
    .unwrap();
    assert!(String::from_utf8_lossy(&plan.writes[0].2).contains("packageVersion=\"2.0.1\""));
    assert!(String::from_utf8_lossy(&plan.writes[1].2).contains("2.0.1"));
}

struct Fake {
    replies: Vec<(&'static str, &'static str, Value)>,
    calls: RefCell<Vec<String>>,
}

impl Remote for Fake {
    fn call(&self, thing: &str, service: &str, _: &Value) -> Result<Value, ServerError> {
        self.calls.borrow_mut().push(format!("{thing}.{service}"));
        self.replies
            .iter()
            .find(|(t, s, _)| *t == thing && *s == service)
            .map(|(_, _, reply)| reply.clone())
            .ok_or_else(|| ServerError::InvalidResponse {
                url: format!("{thing}/{service}"),
                why: "not found".into(),
            })
    }
}

#[test]
fn the_server_view_is_the_frameworks_list_in_its_deploy_order() {
    let remote = Fake {
        replies: vec![
            (
                "PTC.SolutionFramework.Manager",
                "GetBuildingBlockList",
                json!({"rows": [
                    {"buildingBlockName": "Acme.Orders", "buildingBlockType": "Standard", "buildingBlockVersion": "0.9.0", "buildingBlockConfiguredState": "Not Configured", "buildingBlockDeployDate": 1789474430416_i64, "buildingBlockDescription": "d"},
                    {"buildingBlockName": "PTC.Base", "buildingBlockType": "BuildingBlock", "buildingBlockVersion": "9.7.0", "buildingBlockConfiguredState": "Configured"}
                ]}),
            ),
            (
                "PTC.Base.Manager",
                "GetComponentDeploymentOrder",
                json!({"array": ["PTC.Base", "Acme.Orders"]}),
            ),
            (
                "Acme.Orders.EntryPoint",
                "GetComponentDeployConfigurationParameters",
                json!({"apiKey": {"types": ["STRING"], "description": "The key", "optional": false}}),
            ),
        ],
        calls: RefCell::new(Vec::new()),
    };
    let server = server_blocks(&remote).unwrap();
    assert_eq!(
        server
            .iter()
            .map(|b| (b.name.as_str(), b.order))
            .collect::<Vec<_>>(),
        [("PTC.Base", Some(1)), ("Acme.Orders", Some(2))]
    );
    assert_eq!(server[1].deployed, Some(1789474430416));

    let (_dir, solution) = solution();
    let compared = compare(blocks(&solution), &server);
    assert_eq!(
        compared[0].notes,
        [
            "the server has version 0.9.0, the repository 1.0.0",
            "Not Configured on the server: the framework has not run its deploy step"
        ]
    );
    assert_eq!(
        compare(blocks(&solution), &[])[0].notes,
        ["not on the server"]
    );

    let parameters = deploy_parameters(&remote, "Acme.Orders.EntryPoint").unwrap();
    assert_eq!(
        parameters,
        [DeployParameter {
            name: "apiKey".into(),
            types: vec!["STRING".into()],
            description: "The key".into(),
            optional: false,
        }]
    );
    let missing = Fake {
        replies: Vec::new(),
        calls: RefCell::new(Vec::new()),
    };
    assert!(server_blocks(&missing)
        .unwrap_err()
        .starts_with("the server's Solution Framework could not list its building blocks"));
}
