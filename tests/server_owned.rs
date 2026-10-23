//! CLI projections of the shared ownership policy; the server is a local, read-only fake.

use std::io::{Read, Write};
use std::process::Command;

#[test]
fn cli_status_push_and_deploy_expose_owned_parts_without_importing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        // Five entity reads, and each deploy's read of its Project.
        for _ in 0..7 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0; 8192];
            let n = stream.read(&mut buffer).unwrap();
            let request = std::str::from_utf8(&buffer[..n]).unwrap();
            assert!(
                request.starts_with("GET "),
                "a plan must only read the server"
            );
            let xml = "<Group name=\"Acme.Admin_UG\" projectName=\"Acme.Project\"><Members><Member name=\"Acme.Server\"/></Members></Group>";
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{xml}", xml.len()).unwrap();
        }
    });
    std::fs::write(root.join("twaco.toml"), "[[project]]\nname='Acme.Project'\n[[server_owned]]\nentities=['Groups/Acme.*_UG']\nparts=['members']\n").unwrap();
    std::fs::create_dir(root.join("Groups")).unwrap();
    let path = root.join("Groups/Acme.Admin_UG.xml");
    let before = "<Entities><Groups><Group name=\"Acme.Admin_UG\" projectName=\"Acme.Project\"><Members/></Group></Groups></Entities>";
    std::fs::write(&path, before).unwrap();
    std::fs::create_dir_all(root.join(".twaco/profiles")).unwrap();
    std::fs::write(
        root.join(".twaco/profiles/default.toml"),
        format!("url='http://{address}/Thingworx/'\nusername='Acme.User'\npassword='fixture'\n"),
    )
    .unwrap();
    for (args, expected) in [
        (
            vec!["entity", "status", "--all", "--detail"],
            "server-owned  members",
        ),
        (
            vec!["entity", "push", "Acme.Admin_UG"],
            "keeps the server's members",
        ),
        (
            vec!["deploy", "--skip-checks"],
            "keeps the server's members",
        ),
        (vec!["entity", "push", "Acme.Admin_UG", "--json"], ""),
        (vec!["deploy", "--skip-checks", "--json"], ""),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_twaco"))
            .current_dir(root)
            .env_remove("TWACO_LOG")
            .env_remove("TWACO_LOG_FILE")
            .env("TWACO_NO_UPDATE_CHECK", "1")
            .args(&args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = std::str::from_utf8(&output.stdout).unwrap();
        if args.contains(&"--json") {
            let value: serde_json::Value = serde_json::from_str(text).unwrap();
            if args[0] == "deploy" {
                assert_eq!(value["server_owned"][0]["entity"], "Groups/Acme.Admin_UG");
                assert_eq!(
                    value["server_owned"][0]["parts"],
                    serde_json::json!(["members"])
                );
            } else {
                assert_eq!(value["server_owned"], serde_json::json!(["members"]));
            }
        } else {
            assert!(text.contains(expected), "{text}");
        }
    }
    server.join().unwrap();
    assert_eq!(std::fs::read_to_string(path).unwrap(), before);
    assert!(!root.join(".twaco/baselines/default.json").exists());
}
