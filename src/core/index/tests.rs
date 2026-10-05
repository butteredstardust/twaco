use super::*;
use crate::core::catalog;
use crate::core::config::Solution;
use std::path::{Path, PathBuf};

fn key(collection: &str, name: &str) -> EntityKey {
    EntityKey::new(collection, name).unwrap()
}

fn scratch(label: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "twaco-index-{label}-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn write(root: &Path, relative: &str, text: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn entity(
    collection: &str,
    tag: &str,
    name: &str,
    project: &str,
    attributes: &str,
    body: &str,
) -> String {
    format!(
        "<Entities><{collection}><{tag} name=\"{name}\" projectName=\"{project}\"{attributes}>{body}</{tag}></{collection}></Entities>"
    )
}

fn service(name: &str, parameter_shape: Option<&str>, result_shape: Option<&str>) -> String {
    let parameter = parameter_shape
        .map(|shape| {
            format!(
                "<ParameterDefinitions><FieldDefinition name=\"rows\" baseType=\"INFOTABLE\" aspect.dataShape=\"{shape}\"/></ParameterDefinitions>"
            )
        })
        .unwrap_or_default();
    let result = result_shape
        .map(|shape| format!("<ResultType baseType=\"INFOTABLE\" aspect.dataShape=\"{shape}\"/>"))
        .unwrap_or_else(|| "<ResultType baseType=\"STRING\"/>".to_string());
    format!("<ServiceDefinition name=\"{name}\">{parameter}{result}</ServiceDefinition>")
}

fn data_shape(name: &str, base: &str, fields: &[(&str, Option<&str>)]) -> String {
    let fields: String = fields
        .iter()
        .enumerate()
        .map(|(at, (field, shape))| match shape {
            Some(shape) => format!(
                "<FieldDefinition name=\"{field}\" baseType=\"INFOTABLE\" aspect.dataShape=\"{shape}\" ordinal=\"{at}\"/>"
            ),
            None => format!("<FieldDefinition name=\"{field}\" baseType=\"STRING\" ordinal=\"{at}\"/>"),
        })
        .collect();
    let base = if base.is_empty() {
        String::new()
    } else {
        format!(" baseDataShape=\"{base}\"")
    };
    entity(
        "DataShapes",
        "DataShape",
        name,
        "P",
        &base,
        &format!("<FieldDefinitions>{fields}</FieldDefinitions>"),
    )
}

/// One project `P` with a template chain, a shape, DataShapes that type services and a base, and
/// a template cycle.
fn structure() -> (PathBuf, Solution) {
    let root = scratch("structure");
    write(&root, "twaco.toml", "[[project]]\nname = \"P\"\n");
    write(
        &root,
        "ThingShapes/P.Shape.xml",
        &entity(
            "ThingShapes",
            "ThingShape",
            "P.Shape",
            "P",
            "",
            &format!(
                "<ServiceDefinitions>{}</ServiceDefinitions>",
                service("ShapeService", Some("P.Row"), None)
            ),
        ),
    );
    write(
        &root,
        "DataShapes/P.Row.xml",
        &data_shape(
            "P.Row",
            "P.Base",
            &[("id", None), ("child", Some("P.Leaf"))],
        ),
    );
    write(
        &root,
        "DataShapes/P.Base.xml",
        &data_shape("P.Base", "", &[("kind", None)]),
    );
    write(
        &root,
        "DataShapes/P.Leaf.xml",
        &data_shape("P.Leaf", "", &[("value", None)]),
    );
    write(
        &root,
        "ThingTemplates/P.T1.xml",
        &entity(
            "ThingTemplates",
            "ThingTemplate",
            "P.T1",
            "P",
            " baseThingTemplate=\"GenericThing\"",
            &format!(
                "<ThingShape><ServiceDefinitions>{}</ServiceDefinitions></ThingShape>",
                service("Describe", None, Some("P.Row"))
            ),
        ),
    );
    write(
        &root,
        "ThingTemplates/P.T2.xml",
        &entity(
            "ThingTemplates",
            "ThingTemplate",
            "P.T2",
            "P",
            " baseThingTemplate=\"P.T1\"",
            "<ImplementedShapes><ImplementedShape name=\"P.Shape\"/></ImplementedShapes>",
        ),
    );
    write(
        &root,
        "Things/P.A.xml",
        &entity("Things", "Thing", "P.A", "P", " thingTemplate=\"P.T2\"", ""),
    );
    write(
        &root,
        "Things/P.Platform.xml",
        &entity(
            "Things",
            "Thing",
            "P.Platform",
            "P",
            " thingTemplate=\"GenericThing\"",
            "",
        ),
    );
    write(
        &root,
        "ThingTemplates/P.C1.xml",
        &entity(
            "ThingTemplates",
            "ThingTemplate",
            "P.C1",
            "P",
            " baseThingTemplate=\"P.C2\"",
            "",
        ),
    );
    write(
        &root,
        "ThingTemplates/P.C2.xml",
        &entity(
            "ThingTemplates",
            "ThingTemplate",
            "P.C2",
            "P",
            " baseThingTemplate=\"P.C1\"",
            "",
        ),
    );
    let solution = Solution::load(&root.join("twaco.toml")).unwrap();
    (root, solution)
}

fn kinds(index: &Index, from: &str, to: &str) -> Vec<EdgeKind> {
    index
        .edges()
        .into_iter()
        .filter(|(a, b, _)| a == from && b == to)
        .map(|(_, _, edge)| edge.kind)
        .collect()
}

#[test]
fn templates_shapes_and_typed_members_are_edges_and_platform_names_are_not() {
    let (root, solution) = structure();
    let index = Index::build(&solution);
    assert!(index.is_complete(), "{:?}", index.unreadable());
    assert_eq!(
        kinds(&index, "Things/P.A", "ThingTemplates/P.T2"),
        [EdgeKind::Template]
    );
    assert_eq!(
        kinds(&index, "ThingTemplates/P.T2", "ThingTemplates/P.T1"),
        [EdgeKind::Template]
    );
    assert_eq!(
        kinds(&index, "ThingTemplates/P.T2", "ThingShapes/P.Shape"),
        [EdgeKind::ImplementedShape]
    );
    // A service typed by a DataShape: from the service that holds it.
    let typed: Vec<(String, Option<String>)> = index
        .edges()
        .into_iter()
        .filter(|(a, b, e)| {
            a == "ThingShapes/P.Shape" && b == "DataShapes/P.Row" && e.kind == EdgeKind::DataShape
        })
        .map(|(_, _, e)| (e.kind.word().to_string(), e.from_member.clone()))
        .collect();
    assert_eq!(
        typed,
        [("data_shape".to_string(), Some("ShapeService".to_string()))]
    );
    assert_eq!(
        kinds(&index, "ThingTemplates/P.T1", "DataShapes/P.Row"),
        [EdgeKind::DataShape]
    );
    assert_eq!(
        kinds(&index, "DataShapes/P.Row", "DataShapes/P.Base"),
        [EdgeKind::BaseDataShape]
    );
    assert_eq!(
        kinds(&index, "DataShapes/P.Row", "DataShapes/P.Leaf"),
        [EdgeKind::DataShape],
        "a field typed by a DataShape"
    );
    // GenericThing is the platform's, not the repository's: nothing to point at.
    assert!(index
        .edges()
        .iter()
        .all(|(a, b, _)| !(a == "Things/P.Platform") && !b.contains("GenericThing")));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn every_edge_has_the_confidence_of_its_kind() {
    let (root, solution) = structure();
    let index = Index::build(&solution);
    assert!(index
        .edges()
        .iter()
        .all(|(_, _, edge)| edge.confidence() == Confidence::Structural));
    assert!(
        Confidence::Review < Confidence::Resolved && Confidence::Resolved < Confidence::Structural
    );
    assert_eq!(EdgeKind::ScriptReference.confidence(), Confidence::Resolved);
    assert_eq!(EdgeKind::ScriptMention.confidence(), Confidence::Review);
    assert_eq!(Confidence::parse("resolved"), Some(Confidence::Resolved));
    assert_eq!(Confidence::parse("sure"), None);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn inheritance_is_walked_nearest_first_and_shapes_are_implemented_through_templates() {
    let (root, solution) = structure();
    let index = Index::build(&solution);
    assert_eq!(
        index.inheritance_names(&key("Things", "P.A")),
        ["P.T2", "P.Shape", "P.T1", "GenericThing"]
    );
    assert!(index
        .inheritance_names(&key("ThingShapes", "P.Shape"))
        .is_empty());
    assert_eq!(
        index.implementers(&key("ThingShapes", "P.Shape"), None),
        ["P.A", "P.T2"]
    );
    assert!(index
        .implementers(&key("ThingShapes", "P.Shape"), Some("Elsewhere"))
        .is_empty());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn the_index_agrees_with_the_catalog_on_inheritance_for_every_entity() {
    let (root, solution) = structure();
    let index = Index::build(&solution);
    let (entities, skipped) = catalog::inheritance(&solution);
    assert!(skipped.is_empty(), "{skipped:?}");
    assert!(!entities.is_empty());
    for entity in &entities {
        let key = key(&entity.collection, &entity.name);
        assert_eq!(index.inheritance_names(&key), entity.inherits, "{key}");
        if entity.collection == "ThingShapes" {
            assert_eq!(
                index.implementers(&key, None),
                entity.implemented_by,
                "{key}"
            );
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_circle_of_inheritance_is_reported_and_a_tree_is_not() {
    let (root, solution) = structure();
    let index = Index::build(&solution);
    assert_eq!(
        index.inheritance_cycles(),
        [vec![
            "ThingTemplates/P.C1".to_string(),
            "ThingTemplates/P.C2".to_string()
        ]]
    );
    // The walk still ends: a cycle is visited once.
    assert_eq!(
        index.inheritance_names(&key("ThingTemplates", "P.C1")),
        ["P.C2", "P.C1"]
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn dependents_are_found_through_inheritance_with_the_members_involved() {
    let (root, solution) = structure();
    let index = Index::build(&solution);
    let who = index.dependents(&key("ThingTemplates", "P.T1"), &DependentOptions::default());
    let labels: Vec<&str> = who.iter().map(|d| d.label.as_str()).collect();
    assert_eq!(
        labels,
        ["ThingTemplates/P.T2", "Things/P.A"],
        "nearest first"
    );
    assert_eq!((who[0].depth, who[1].depth), (1, 2));
    assert!(who.iter().all(|d| d.confidence == Confidence::Structural));
    assert_eq!(who[1].path.len(), 2);
    assert_eq!(
        who[1].path[0].to, "ThingTemplates/P.T1",
        "the chain starts at the thing asked about"
    );

    // Who is affected if the DataShape P.Leaf changes: the shape that has a field of it, and everything typed by that shape.
    let leaf = index.dependents(&key("DataShapes", "P.Leaf"), &DependentOptions::default());
    let leaf: Vec<&str> = leaf.iter().map(|d| d.label.as_str()).collect();
    assert_eq!(
        leaf,
        [
            "DataShapes/P.Row",
            "ThingShapes/P.Shape",
            "ThingTemplates/P.T1",
            "ThingTemplates/P.T2",
            "Things/P.A"
        ]
    );

    // Asking about one member narrows what is followed: nothing names Describe, but inheritance carries it.
    let describe = index.dependents(
        &key("ThingTemplates", "P.T1"),
        &DependentOptions {
            member: Some("Describe".into()),
            ..Default::default()
        },
    );
    assert_eq!(
        describe
            .iter()
            .map(|d| d.label.as_str())
            .collect::<Vec<_>>(),
        ["ThingTemplates/P.T2", "Things/P.A"]
    );
    let limited = index.dependents(
        &key("ThingTemplates", "P.T1"),
        &DependentOptions {
            max_depth: Some(1),
            ..Default::default()
        },
    );
    assert_eq!(limited.len(), 1);
    assert!(index
        .dependents(&key("Things", "P.Nope"), &DependentOptions::default())
        .is_empty());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn references_from_an_entity_come_strongest_first() {
    let (root, solution) = structure();
    let index = Index::build(&solution);
    let from: Vec<(String, EdgeKind)> = index
        .references_from(&key("ThingTemplates", "P.T2"))
        .into_iter()
        .map(|step| (step.to, step.kind))
        .collect();
    assert!(from.contains(&("ThingTemplates/P.T1".to_string(), EdgeKind::Template)));
    assert!(from.contains(&(
        "ThingShapes/P.Shape".to_string(),
        EdgeKind::ImplementedShape
    )));
    let reach = index.reachable_from(&[key("Things", "P.A")], Confidence::Review);
    assert!(reach.contains(&key("DataShapes", "P.Leaf")), "{reach:?}");
    assert!(!reach.contains(&key("Things", "P.Platform")));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn projects_are_nodes_with_their_dependencies_and_a_deploy_order() {
    let root = scratch("projects");
    write(
        &root,
        "twaco.toml",
        "[[project]]\nname = \"Base\"\nroot = \"base\"\n\n[[project]]\nname = \"App\"\nroot = \"app\"\ndepends_on = [\"Base\"]\n",
    );
    write(
        &root,
        "base/ThingTemplates/Base.T.xml",
        &entity("ThingTemplates", "ThingTemplate", "Base.T", "Base", "", ""),
    );
    write(
        &root,
        "app/Things/App.A.xml",
        &entity(
            "Things",
            "Thing",
            "App.A",
            "App",
            " thingTemplate=\"Base.T\"",
            "",
        ),
    );
    let solution = Solution::load(&root.join("twaco.toml")).unwrap();
    let index = Index::build(&solution);
    assert_eq!(index.deploy_order(), ["Base", "App"]);
    assert_eq!(
        kinds(&index, "project App", "project Base"),
        [EdgeKind::ProjectDependency]
    );
    assert_eq!(
        kinds(&index, "Things/App.A", "ThingTemplates/Base.T"),
        [EdgeKind::Template]
    );
    let affected = index.projects_of(&["Things/App.A".to_string(), "project Base".to_string()]);
    assert_eq!(affected, [(0, "Base".to_string()), (1, "App".to_string())]);
    assert_eq!(index.node(&key("Things", "App.A")).unwrap().project, "App");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_document_that_cannot_be_read_is_listed_and_does_not_stop_the_build() {
    let (root, solution) = structure();
    write(
        &root,
        "Things/P.Broken.xml",
        "<Entities><Things><Thing name=\"P.Broken\" projectName=\"P\"",
    );
    let index = Index::build(&solution);
    assert!(!index.is_complete());
    assert!(
        index
            .unreadable()
            .iter()
            .any(|entry| entry.what.contains("P.Broken")),
        "{:?}",
        index.unreadable()
    );
    // Everything else is still there.
    assert!(index.contains(&key("Things", "P.A")));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn an_entity_defined_twice_keeps_the_first_and_says_so() {
    let (root, solution) = structure();
    // The same Thing filed in a second place under the same project.
    write(
        &root,
        "Things/Copy/P.A.xml",
        &entity("Things", "Thing", "P.A", "P", " thingTemplate=\"P.T1\"", ""),
    );
    let index = Index::build(&solution);
    assert!(
        index
            .unreadable()
            .iter()
            .any(|entry| entry.why.contains("is also defined in")),
        "{:?}",
        index.unreadable()
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn the_bundled_repository_has_the_structure_its_files_declare() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/corpus/acme-orders");
    let solution = Solution::load(&root.join("twaco.toml")).unwrap();
    let index = Index::build(&solution);
    assert!(index.is_complete(), "{:?}", index.unreadable());
    assert_eq!(index.deploy_order(), ["Acme.Orders"]);
    assert_eq!(
        kinds(
            &index,
            "Things/Acme.Orders.Manager",
            "ThingTemplates/Acme.Orders.Base_TT"
        ),
        [EdgeKind::Template]
    );
    assert_eq!(
        kinds(
            &index,
            "Things/Acme.Orders.Audit",
            "ThingShapes/Acme.Orders.Audit_TS"
        ),
        [EdgeKind::ImplementedShape]
    );
    let users = index.dependents(
        &key("DataShapes", "Acme.Orders.OrderLine_DS"),
        &DependentOptions::default(),
    );
    let manager = users
        .iter()
        .find(|d| d.label == "Things/Acme.Orders.Manager")
        .expect("the manager's services return order lines");
    assert!(
        manager.members.contains(&"GetOrder".to_string())
            && manager.members.contains(&"LoadLines".to_string()),
        "{manager:?}"
    );
}
