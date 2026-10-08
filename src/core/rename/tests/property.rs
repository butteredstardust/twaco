use super::*;

fn property_fixture(tag: &str) -> Fixture {
    let root_guard = tempfile::Builder::new()
        .prefix(&format!("twaco-rename-prop-{tag}-"))
        .tempdir()
        .unwrap();
    let root = root_guard.path().to_path_buf();
    std::fs::create_dir_all(&root).unwrap();
    write(&root, "twaco.toml", "[[project]]\nname = \"P\"\ncollections = [\"ThingShapes\", \"ThingTemplates\", \"Things\"]\n");
    write(&root, "ThingShapes/P.Shape.xml", "<Entities><ThingShapes><ThingShape name=\"P.Shape\" projectName=\"P\"><PropertyDefinitions><PropertyDefinition name=\"Level\" baseType=\"NUMBER\"/></PropertyDefinitions><ServiceImplementations><ServiceImplementation name=\"Read\"><ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row><code><![CDATA[return me.Level;]]></code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations></ThingShape></ThingShapes></Entities>\n");
    write(
        &root,
        "src/P.Shape/services/Read/script.js",
        "return me.Level;",
    );
    write(&root, "ThingTemplates/P.Base.xml", "<Entities><ThingTemplates><ThingTemplate name=\"P.Base\" projectName=\"P\" baseThingTemplate=\"GenericThing\"><ImplementedShapes><ImplementedShape name=\"P.Shape\"/></ImplementedShapes></ThingTemplate></ThingTemplates></Entities>\n");
    write(&root, "Things/P.One.xml", "\u{feff}<Entities>\r\n<Things><Thing name=\"P.One\" projectName=\"P\" thingTemplate=\"P.Base\"><ThingProperties><Level><Value>3</Value></Level></ThingProperties></Thing></Things></Entities>\r\n");
    write(&root, "Things/P.Other.xml", "<Entities><Things><Thing name=\"P.Other\" projectName=\"P\" thingTemplate=\"GenericThing\"><ThingShape><PropertyDefinitions><PropertyDefinition name=\"Level\" baseType=\"NUMBER\"/></PropertyDefinitions></ThingShape><ThingProperties><Level><Value>9</Value></Level></ThingProperties></Thing></Things></Entities>\n");
    write(
        &root,
        "src/P.One/services/Use/script.js",
        "const a = Things[\"P.One\"].Level; const b = Things[\"P.Other\"].Level; me.Level = 1;",
    );
    let solution = Solution::load(&root.join(CONFIG_FILE)).unwrap();
    Fixture {
        _dir: root_guard,
        root,
        solution,
    }
}

fn property_spec(scope: &str, old: &str, new: &str) -> Spec {
    Spec {
        kind: Kind::Property,
        old: old.to_string(),
        new: new.to_string(),
        scope: Some(scope.to_string()),
        service: None,
    }
}

#[test]
fn a_property_rename_follows_the_shape_its_implementers_and_their_values_and_round_trips() {
    let fixture = property_fixture("all");
    let before = snapshot(&fixture.root);
    let planned = plan(
        &fixture.solution,
        &property_spec("P.Shape", "Level", "Height"),
    )
    .unwrap();
    assert_eq!(snapshot(&fixture.root), before, "a plan writes nothing");
    apply(
        &fixture.solution,
        &planned,
        &options(false),
        &locked(&fixture),
    )
    .unwrap();
    let read = |path: &str| std::fs::read_to_string(fixture.root.join(path)).unwrap();
    assert!(
        read("ThingShapes/P.Shape.xml").contains("PropertyDefinition name=\"Height\"")
            && read("ThingShapes/P.Shape.xml").contains("return me.Height;")
    );
    assert!(read("src/P.Shape/services/Read/script.js").contains("me.Height"));
    // A Thing's value is typed by its template's property: the element follows, with its BOM and CRLF.
    assert!(
        read("Things/P.One.xml").contains("<Height><Value>3</Value></Height>")
            && read("Things/P.One.xml").starts_with('\u{feff}')
    );
    // An unrelated Thing with its own property of the same name is not this property.
    assert!(read("Things/P.Other.xml").contains("<Level><Value>9</Value></Level>"));
    // A lookup of an affected Thing follows, and so does `me` inside that Thing's own script; another Thing's does not.
    assert_eq!(
        read("src/P.One/services/Use/script.js"),
        "const a = Things[\"P.One\"].Height; const b = Things[\"P.Other\"].Level; me.Height = 1;"
    );
    // Back, byte for byte (the ledger is the only addition).
    let back = plan(
        &fixture.solution,
        &property_spec("P.Shape", "Height", "Level"),
    )
    .unwrap();
    apply(&fixture.solution, &back, &options(false), &locked(&fixture)).unwrap();
    assert_eq!(snapshot_without_twaco(&fixture.root), before);
}

#[test]
fn a_property_rename_refuses_an_inherited_declaration_a_taken_name_and_an_unknown_property() {
    let fixture = property_fixture("refuse");
    // Declared on the shape: renaming it from the template is refused, naming the shape.
    let inherited = plan(
        &fixture.solution,
        &property_spec("P.Base", "Level", "Height"),
    )
    .unwrap_err();
    assert!(
        inherited.to_string().contains("declared on P.Shape"),
        "{inherited}"
    );
    let missing = plan(
        &fixture.solution,
        &property_spec("P.Shape", "Nope", "Height"),
    )
    .unwrap_err();
    assert!(
        missing
            .to_string()
            .contains("declares no property named Nope"),
        "{missing}"
    );
    // The name is already a property of an entity the rename reaches.
    write(&fixture.root, "Things/P.One.xml", "<Entities><Things><Thing name=\"P.One\" projectName=\"P\" thingTemplate=\"P.Base\"><ThingShape><PropertyDefinitions><PropertyDefinition name=\"Height\" baseType=\"NUMBER\"/></PropertyDefinitions></ThingShape></Thing></Things></Entities>\n");
    let solution = Solution::load(&fixture.root.join(CONFIG_FILE)).unwrap();
    let taken = plan(&solution, &property_spec("P.Shape", "Level", "Height")).unwrap_err();
    assert!(matches!(taken, RenameError::Exists { .. }), "{taken}");
    assert!(plan(&solution, &property_spec("P.Shape", "Level", "1bad")).is_err());
    assert!(matches!(
        plan(&solution, &property_spec("P.Shape", "Level", "Level")),
        Err(RenameError::Same { .. })
    ));
}
