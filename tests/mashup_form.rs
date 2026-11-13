//! A record form's initial selection advisory and save outcomes through the CLI, offline.

use serde_json::{json, Value};
use std::process::Command;

#[test]
fn form_bind_warns_and_save_messages_plan_then_wire_both_outcomes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("twaco.toml"), "[[project]]\nname = 'Acme'\n").unwrap();
    std::fs::create_dir_all(root.join("Things")).unwrap();
    std::fs::create_dir_all(root.join("DataShapes")).unwrap();
    std::fs::write(root.join("Things/Acme.Manager.xml"), r#"<Entities><Things><Thing name="Acme.Manager" projectName="Acme"><ThingShape><ServiceDefinitions><ServiceDefinition name="GetRecord"><ResultType baseType="INFOTABLE" aspect.dataShape="Acme.Record_DS"/></ServiceDefinition><ServiceDefinition name="Save"><ResultType baseType="NOTHING"/></ServiceDefinition></ServiceDefinitions></ThingShape></Thing></Things></Entities>"#).unwrap();
    std::fs::write(root.join("DataShapes/Acme.Record_DS.xml"), r#"<Entities><DataShapes><DataShape name="Acme.Record_DS" projectName="Acme"><FieldDefinitions><FieldDefinition name="note" baseType="STRING"/></FieldDefinitions></DataShape></DataShapes></Entities>"#).unwrap();
    let run = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_twaco"))
            .current_dir(root)
            .env_clear()
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    };
    run(&["mashup", "new", "Acme.Form_MU", "--apply"]);
    run(&["mashup", "add", "Acme.Form_MU", "textfield", "--apply"]);
    let binding = run(&[
        "mashup",
        "bind",
        "Acme.Form_MU",
        "--from",
        "data:Acme.Manager.GetRecord[SelectedRows].note",
        "--to",
        "ui:ptcstextfield-3.Text",
        "--apply",
        "--json",
    ]);
    let binding: Value = serde_json::from_slice(&binding.stdout).unwrap();
    assert!(binding["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning.as_str().unwrap().contains("one-row result")));
    run(&[
        "mashup",
        "source",
        "add",
        "Acme.Form_MU",
        "Acme.Manager",
        "Save",
        "--as",
        "save",
        "--apply",
    ]);
    let entity = root.join("Mashups/Acme.Form_MU.xml");
    let before = std::fs::read(&entity).unwrap();
    let args = [
        "mashup",
        "recipe",
        "message",
        "Acme.Form_MU",
        "--after",
        "data:Acme.Manager.Save[as save]",
        "--say",
        "Saved",
        "--on-failure",
        "Save failed",
        "--json",
    ];
    let planned: Value = serde_json::from_slice(&run(&args).stdout).unwrap();
    assert_eq!(planned["applied"], false);
    assert_eq!(std::fs::read(&entity).unwrap(), before);
    let mut apply = args.to_vec();
    apply.push("--apply");
    let applied: Value = serde_json::from_slice(&run(&apply).stdout).unwrap();
    assert_eq!(applied["applied"], true);
    assert_eq!(applied["added"].as_array().unwrap().len(), 2);
    run(&[
        "mashup",
        "recipe",
        "close-after",
        "Acme.Form_MU",
        "--after",
        "data:Acme.Manager.Save[as save]",
        "--apply",
    ]);
    let description: Value =
        serde_json::from_slice(&run(&["mashup", "describe", "Acme.Form_MU", "--json"]).stdout)
            .unwrap();
    let events = description["events"].as_array().unwrap();
    assert!(events
        .iter()
        .any(|event| event["trigger"] == "data:Acme.Manager.Save[as save].Data_Error"));
    assert!(events
        .iter()
        .any(|event| event["handler"] == "mashup:CloseIfPopup"
            && event["trigger"] == "data:Acme.Manager.Save[as save].ServiceInvokeCompleted"));
    assert!(!events
        .iter()
        .any(|event| event["handler"] == "mashup:CloseIfPopup"
            && event["trigger"] == "data:Acme.Manager.Save[as save].Data_Error"));
    let content: Value = serde_json::from_slice(
        &std::fs::read(root.join("src/Acme.Form_MU/mashup/content.json")).unwrap(),
    )
    .unwrap();
    let messages: Vec<_> = content["UI"]["Widgets"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|widget| widget["Properties"]["Type"] == "statusmessage")
        .map(|widget| {
            json!([
                widget["Properties"]["Message"],
                widget["Properties"]["MessageType"]
            ])
        })
        .collect();
    assert_eq!(
        messages,
        vec![json!(["Saved", "info"]), json!(["Save failed", "error"])]
    );
}
