//! The lazy-credential regression: offline commands must run in a genuinely empty environment.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn offline_projects_needs_no_profile_or_environment() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("twaco-offline-{}-{nonce}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"Offline\"\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_twaco"))
        .arg("projects")
        .current_dir(&root)
        .env_clear()
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!root.join(".twaco/profiles/default.toml").exists());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn types_command_writes_shared_declarations_and_second_run_is_clean() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "twaco-types-command-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(root.join("Things")).unwrap();
    std::fs::create_dir_all(root.join("DataShapes")).unwrap();
    std::fs::create_dir_all(root.join("src/T/services/Run")).unwrap();
    std::fs::create_dir_all(root.join("src/T/services/Stale")).unwrap();
    std::fs::write(
        root.join("twaco.toml"),
        "[[project]]\nname = \"P\"\ncollections = [\"Things\", \"DataShapes\"]\n",
    )
    .unwrap();
    std::fs::write(
        root.join(".gitignore"),
        ".twaco/types/\n**/services/*/jsconfig.json\n**/services/*/twaco-globals.d.ts\n",
    )
    .unwrap();
    std::fs::write(
        root.join("Things/T.xml"),
        "<Entities><Things><Thing name=\"T\" projectName=\"P\" thingTemplate=\"GenericThing\"><ThingShape>\
         <PropertyDefinitions><PropertyDefinition name=\"Rows\" baseType=\"INFOTABLE\" aspect.dataShape=\"Rows\"/></PropertyDefinitions>\
         <ServiceDefinitions><ServiceDefinition name=\"Run\"><ParameterDefinitions>\
         <FieldDefinition name=\"count\" baseType=\"NUMBER\" description=\"How many\"/>\
         </ParameterDefinitions><ResultType baseType=\"STRING\"/></ServiceDefinition></ServiceDefinitions>\
         </ThingShape></Thing></Things></Entities>",
    )
    .unwrap();
    std::fs::write(
        root.join("src/T/services/Run/script.js"),
        "result = count;\n",
    )
    .unwrap();
    std::fs::write(root.join("src/T/services/Stale/script.js"), "stale();\n").unwrap();
    std::fs::write(
        root.join("Things/Broken.xml"),
        "<Entities><Things><Thing name=\"Broken\"",
    )
    .unwrap();
    std::fs::write(
        root.join("DataShapes/Rows.xml"),
        "<Entities><DataShapes><DataShape name=\"Rows\" projectName=\"P\"><FieldDefinitions>\
         <FieldDefinition name=\"Value\" baseType=\"NUMBER\"/>\
         </FieldDefinitions></DataShape></DataShapes></Entities>",
    )
    .unwrap();

    let run = || {
        Command::new(env!("CARGO_BIN_EXE_twaco"))
            .arg("types")
            .current_dir(&root)
            .env_clear()
            .output()
            .unwrap()
    };
    let first = run();
    assert!(
        first.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(String::from_utf8_lossy(&first.stderr).contains("skipped"));
    assert!(String::from_utf8_lossy(&first.stdout)
        .contains("typed 1 entities, 1 DataShapes and 1 services (6 files written)"));
    let directory = root.join(".twaco/types");
    let before: Vec<Vec<u8>> = [
        "twx.d.ts",
        "datashapes.d.ts",
        "entities.d.ts",
        "collections.d.ts",
    ]
    .iter()
    .map(|name| std::fs::read(directory.join(name)).unwrap())
    .collect();
    assert!(String::from_utf8_lossy(&before[2]).contains("Rows: twx.INFOTABLE<twx.ds.D_Rows>"));
    let project = root.join("src/T/services/Run");
    assert!(std::fs::read_to_string(project.join("jsconfig.json"))
        .unwrap()
        .contains("../../../../.twaco/types/*.d.ts"));
    let globals = std::fs::read_to_string(project.join("twaco-globals.d.ts")).unwrap();
    assert!(globals.contains("declare const me: twx.E_T;"));
    assert!(globals.contains("/** How many */\ndeclare let count: number;"));
    assert!(globals.contains("declare let result: string;"));
    assert!(!root.join("src/T/services/Stale/jsconfig.json").exists());

    let second = run();
    assert!(
        second.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert!(String::from_utf8_lossy(&second.stdout).contains("(0 files written)"));
    let after: Vec<Vec<u8>> = [
        "twx.d.ts",
        "datashapes.d.ts",
        "entities.d.ts",
        "collections.d.ts",
    ]
    .iter()
    .map(|name| std::fs::read(directory.join(name)).unwrap())
    .collect();
    assert_eq!(after, before);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn sync_check_relayout_reports_without_writing_then_the_migration_settles() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("twaco-relayout-{}-{nonce}", std::process::id()));
    let entity_dir = root.join("Things");
    let service_dir = root.join("src/T/services/S");
    std::fs::create_dir_all(&entity_dir).unwrap();
    std::fs::create_dir_all(&service_dir).unwrap();
    std::fs::write(
        root.join("twaco.toml"),
        "[format]\nindent_cdata_payload = false\n\n[[project]]\nname = \"P\"\ncollections = [\"Things\"]\n",
    )
    .unwrap();
    let entity = b"<Entities><Things><Thing name=\"T\" projectName=\"P\"><ThingShape>\
<ServiceDefinitions><ServiceDefinition name=\"S\"></ServiceDefinition></ServiceDefinitions>\
<ServiceImplementations><ServiceImplementation name=\"S\" handlerName=\"Script\">\
<ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row>\
<code><![CDATA[\n            var a = 1;\n            ]]></code>\
</Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations>\
</ThingShape></Thing></Things></Entities>";
    let entity_path = entity_dir.join("T.xml");
    std::fs::write(&entity_path, entity).unwrap();
    std::fs::write(
        service_dir.join("definition.xml"),
        "<ServiceDefinition name=\"S\"></ServiceDefinition>\n",
    )
    .unwrap();
    std::fs::write(service_dir.join("script.js"), "var a = 1;").unwrap();

    let check = Command::new(env!("CARGO_BIN_EXE_twaco"))
        .args(["sync", "--all", "--check", "--relayout"])
        .current_dir(&root)
        .env_clear()
        .output()
        .unwrap();
    assert_eq!(
        check.status.code(),
        Some(1),
        "the migration should be reported as drift"
    );
    assert_eq!(
        std::fs::read(&entity_path).unwrap(),
        entity,
        "--check must not write"
    );

    let apply = Command::new(env!("CARGO_BIN_EXE_twaco"))
        .args(["sync", "--all", "--relayout"])
        .current_dir(&root)
        .env_clear()
        .output()
        .unwrap();
    assert!(
        apply.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&apply.stderr)
    );
    let migrated = std::fs::read_to_string(&entity_path).unwrap();
    assert!(migrated.contains("<code><![CDATA[\nvar a = 1;\n]]></code>"));

    let settled = Command::new(env!("CARGO_BIN_EXE_twaco"))
        .args(["sync", "--all", "--check", "--relayout"])
        .current_dir(&root)
        .env_clear()
        .output()
        .unwrap();
    assert!(
        settled.status.success(),
        "a second relayout check must be clean"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_writing_command_is_refused_while_another_holds_the_workspace() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("twaco-locked-{}-{nonce}", std::process::id()));
    std::fs::create_dir_all(root.join("Things")).unwrap();
    std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
    std::fs::write(
        root.join("Things/T.xml"),
        "<Entities><Things><Thing name=\"T\" projectName=\"P\"></Thing></Things></Entities>",
    )
    .unwrap();

    // This test process plays the other command, a deploy in progress.
    let held = twaco::core::lock::acquire(&root, "deploy", &[]).unwrap();
    let twaco = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_twaco"))
            .args(args)
            .current_dir(&root)
            .env_clear()
            .output()
            .unwrap()
    };

    let writing = twaco(&["sync", "--all"]);
    assert_eq!(writing.status.code(), Some(2));
    let said = String::from_utf8_lossy(&writing.stderr);
    assert!(
        said.contains("another twaco command") && said.contains("twaco deploy"),
        "{said}"
    );

    let reading = twaco(&["sync", "--all", "--check"]);
    assert_ne!(
        reading.status.code(),
        Some(2),
        "a read-only run is not blocked: {}",
        String::from_utf8_lossy(&reading.stderr)
    );

    let types = twaco(&["types"]);
    assert_eq!(
        types.status.code(),
        Some(2),
        "types must take the workspace lock"
    );

    drop(held);
    let free = twaco(&["sync", "--all"]);
    assert!(!String::from_utf8_lossy(&free.stderr).contains("another twaco command"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_service_sidecar_missing_its_script_fails_the_sidecars_gate() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("twaco-incomplete-{}-{nonce}", std::process::id()));
    let service_dir = root.join("src/T/services/S");
    std::fs::create_dir_all(root.join("Things")).unwrap();
    std::fs::create_dir_all(&service_dir).unwrap();
    std::fs::write(
        root.join("twaco.toml"),
        "[[project]]\nname = \"P\"\ncollections = [\"Things\"]\n",
    )
    .unwrap();
    std::fs::write(
        root.join("Things/T.xml"),
        "<Entities><Things><Thing name=\"T\" projectName=\"P\"><ThingShape>\
<ServiceDefinitions><ServiceDefinition name=\"S\"></ServiceDefinition></ServiceDefinitions>\
<ServiceImplementations><ServiceImplementation name=\"S\" handlerName=\"Script\">\
<ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row>\
<code><![CDATA[\nvar a = 1;\n]]></code>\
</Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations>\
</ThingShape></Thing></Things></Entities>",
    )
    .unwrap();
    // The definition is there; the script was deleted.
    std::fs::write(
        service_dir.join("definition.xml"),
        "<ServiceDefinition name=\"S\"></ServiceDefinition>\n",
    )
    .unwrap();

    let check = Command::new(env!("CARGO_BIN_EXE_twaco"))
        .args(["check", "--detail"])
        .current_dir(&root)
        .env_clear()
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&check.stdout);
    assert!(!check.status.success(), "stdout={stdout}");
    assert!(
        stdout.contains("incomplete") && stdout.contains("script.js"),
        "stdout={stdout}"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn the_line_endings_gate_skips_what_git_ignores() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    // The solution sits in a folder whose own .gitignore would hide notes.md: it must not count.
    let outer = std::env::temp_dir().join(format!("twaco-ignored-{}-{nonce}", std::process::id()));
    std::fs::create_dir_all(&outer).unwrap();
    std::fs::write(outer.join(".gitignore"), "notes.md\n").unwrap();
    let root = outer.join("solution");
    std::fs::create_dir_all(root.join("Things")).unwrap();
    std::fs::create_dir_all(root.join("raw")).unwrap();
    std::fs::write(
        root.join("twaco.toml"),
        "[[project]]\nname = \"P\"\ncollections = [\"Things\"]\n",
    )
    .unwrap();
    std::fs::write(root.join(".gitignore"), "raw/\n").unwrap();
    std::fs::write(root.join("raw/config.yaml"), "a: 1\r\nb: 2\n").unwrap();
    std::fs::write(root.join("notes.md"), "one\r\ntwo\n").unwrap();
    // A ripgrep-style .ignore excludes what git still tracks, such as verbatim exports.
    std::fs::create_dir_all(root.join("exports")).unwrap();
    std::fs::write(root.join(".ignore"), "exports/\n").unwrap();
    std::fs::write(root.join("exports/E.xml"), "<a>\r\n</a>\n").unwrap();

    let check = Command::new(env!("CARGO_BIN_EXE_twaco"))
        .args(["check", "--detail"])
        .current_dir(&root)
        .env_clear()
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&check.stdout);
    assert!(
        stdout.contains("notes.md [line endings/mixed]"),
        "a tracked file is still checked: {stdout}"
    );
    assert!(
        !stdout.contains("config.yaml"),
        "an ignored file is not: {stdout}"
    );
    assert!(!stdout.contains("E.xml"), ".ignore is honoured: {stdout}");
    let _ = std::fs::remove_dir_all(outer);
}

#[test]
fn bundle_carries_10_2_ai_entities_and_refuses_an_unknown_collection() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("twaco-collections-{}-{nonce}", std::process::id()));
    for dir in ["Things", "AIAgents", "MCPNamespaces"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
    let entity = |collection: &str, tag: &str, name: &str| {
        format!("<Entities><{collection}><{tag} name=\"{name}\" projectName=\"P\"></{tag}></{collection}></Entities>")
    };
    std::fs::write(root.join("Things/T.xml"), entity("Things", "Thing", "T")).unwrap();
    let twaco_here = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_twaco"))
            .args(args)
            .current_dir(&root)
            .env_clear()
            .output()
            .unwrap()
    };
    // Without agents, the 10.2 collections are not written at all: a 10.1 server does not know them.
    assert!(twaco_here(&["bundle"]).status.success());
    let plain = std::fs::read_to_string(root.join("dist/bundle.xml")).unwrap();
    assert!(
        plain.contains("<Things>")
            && !plain.contains("AIAgents")
            && !plain.contains("MCPNamespaces"),
        "{plain}"
    );
    std::fs::write(
        root.join("AIAgents/A.xml"),
        entity("AIAgents", "AIAgent", "A"),
    )
    .unwrap();
    std::fs::write(
        root.join("MCPNamespaces/N.xml"),
        entity("MCPNamespaces", "MCPNamespace", "N"),
    )
    .unwrap();
    let twaco = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_twaco"))
            .args(args)
            .current_dir(&root)
            .env_clear()
            .output()
            .unwrap()
    };

    let built = twaco(&["bundle"]);
    assert!(
        built.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&built.stderr)
    );
    let bundle = std::fs::read_to_string(root.join("dist/bundle.xml")).unwrap();
    let at = |needle: &str| {
        bundle
            .find(needle)
            .unwrap_or_else(|| panic!("{needle} missing:\n{bundle}"))
    };
    assert!(at("<Thing name=\"T\"") < at("<MCPNamespace name=\"N\""));
    assert!(at("<MCPNamespace name=\"N\"") < at("<AIAgent name=\"A\""));

    // Filed in a folder not named for its collection: still a Thing, still bundled.
    std::fs::create_dir_all(root.join("DataTables")).unwrap();
    std::fs::write(
        root.join("DataTables/D.xml"),
        entity("Things", "Thing", "D"),
    )
    .unwrap();
    assert!(twaco(&["bundle"]).status.success());
    let bundle = std::fs::read_to_string(root.join("dist/bundle.xml")).unwrap();
    assert!(bundle.contains("<Thing name=\"D\""), "{bundle}");

    std::fs::create_dir_all(root.join("Timers")).unwrap();
    std::fs::write(root.join("Timers/G.xml"), entity("Gizmos", "Gizmo", "G")).unwrap();
    let refused = twaco(&["bundle"]);
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(
        !refused.status.success() && said.contains("<Gizmos>"),
        "an unknown collection must not vanish: {said}"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn an_advisory_gate_reports_without_failing_the_run() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("twaco-advisory-{}-{nonce}", std::process::id()));
    let service_dir = root.join("src/T/services/S");
    std::fs::create_dir_all(root.join("Things")).unwrap();
    std::fs::create_dir_all(&service_dir).unwrap();
    // A for-in loop: a script trap, in a script that is otherwise formatted and in sync.
    let script = "for (var k in obj) {\n    logger.info(k);\n}\n";
    std::fs::write(
        root.join("Things/T.xml"),
        format!(
            "<Entities><Things><Thing name=\"T\" projectName=\"P\"><ThingShape>\
<ServiceDefinitions><ServiceDefinition name=\"S\"></ServiceDefinition></ServiceDefinitions>\
<ServiceImplementations><ServiceImplementation name=\"S\" handlerName=\"Script\">\
<ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row>\
<code><![CDATA[\n{script}]]></code>\
</Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations>\
</ThingShape></Thing></Things></Entities>"
        ),
    )
    .unwrap();
    std::fs::write(
        service_dir.join("definition.xml"),
        "<ServiceDefinition name=\"S\"></ServiceDefinition>\n",
    )
    .unwrap();
    std::fs::write(service_dir.join("script.js"), script).unwrap();
    let project = "[[project]]\nname = \"P\"\ncollections = [\"Things\"]\n";
    // Settle formatting and the sidecar the way twaco writes them, so only the trap remains.
    std::fs::write(root.join("twaco.toml"), project).unwrap();
    for args in [&["fmt"][..], &["sync", "--all"][..]] {
        Command::new(env!("CARGO_BIN_EXE_twaco"))
            .args(args)
            .current_dir(&root)
            .env_clear()
            .output()
            .unwrap();
    }
    let check = |config: &str| {
        std::fs::write(root.join("twaco.toml"), config).unwrap();
        Command::new(env!("CARGO_BIN_EXE_twaco"))
            .args(["check"])
            .current_dir(&root)
            .env_clear()
            .output()
            .unwrap()
    };

    let blocking = check(project);
    let said = String::from_utf8_lossy(&blocking.stdout);
    assert!(
        !blocking.status.success() && said.contains("FAIL    script traps"),
        "{said}"
    );

    let advisory = check(&format!(
        "[gates]\nadvisory = [\"script traps\"]\n\n{project}"
    ));
    let said = String::from_utf8_lossy(&advisory.stdout);
    assert!(
        advisory.status.success(),
        "an advisory gate must not fail the run: {said}"
    );
    assert!(
        said.contains("warn    script traps"),
        "its findings are still reported: {said}"
    );

    let live = check(&format!(
        "[gates]\nadvisory = [\"live parse\"]\n\n{project}"
    ));
    let said = String::from_utf8_lossy(&live.stderr);
    // The fixture's script trap blocks anyway, so the refusal itself is what is asserted.
    assert!(
        !live.status.success() && said.contains("live parse") && said.contains("not a gate"),
        "deploy always parses, so the live parse cannot be advisory: {said}"
    );
    let typo = check(&format!(
        "[gates]\nadvisory = [\"script trap\"]\n\n{project}"
    ));
    let said = String::from_utf8_lossy(&typo.stderr);
    assert!(
        !typo.status.success() && said.contains("not a gate"),
        "a misspelt gate is refused: {said}"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_reader_that_stops_early_is_not_a_crash() {
    // Like `twaco guide | head -1`: the reader closes the pipe before twaco has written.
    let mut child = Command::new(env!("CARGO_BIN_EXE_twaco"))
        // Long enough that twaco is still writing when the reader leaves, whatever the buffer.
        .args(["guide", "service-code"])
        .env_clear()
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let output = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("panicked"),
        "a closed pipe is not a panic: {stderr}"
    );
    // 141 is a shell's closed-pipe status: twaco cannot know the rest of its output was wanted,
    // so it must not claim success.
    assert_eq!(output.status.code(), Some(141), "{stderr}");
}

struct RenameFixture {
    root: PathBuf,
}

impl Drop for RenameFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn rename_fixture(tag: &str) -> RenameFixture {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("twaco-rename-{tag}-{}-{nonce}", std::process::id()));
    for directory in ["Things", "Projects", "src/A.Manager", "docs"] {
        std::fs::create_dir_all(root.join(directory)).unwrap();
    }
    std::fs::write(
        root.join("twaco.toml"),
        "[[project]]\nname = \"A\"\ncollections = [\"Things\", \"Projects\"]\n",
    )
    .unwrap();
    std::fs::write(
        root.join("Things/A.Manager.xml"),
        "<Entities><Things><Thing name=\"A.Manager\" projectName=\"A\"><Description>A.Manager</Description></Thing></Things></Entities>\n",
    ).unwrap();
    std::fs::write(
        root.join("Things/A.Manager.Child.xml"),
        "<Entities><Things><Thing name=\"A.Manager.Child\" projectName=\"A\"></Thing></Things></Entities>\n",
    ).unwrap();
    std::fs::write(
        root.join("Things/Taken.xml"),
        "<Entities><Things><Thing name=\"Taken\" projectName=\"A\"></Thing></Things></Entities>\n",
    )
    .unwrap();
    std::fs::write(
        root.join("Projects/A.xml"),
        "<Entities><Projects><Project name=\"A\" projectName=\"A\"></Project></Projects></Entities>\n",
    ).unwrap();
    std::fs::write(
        root.join("src/A.Manager/note.txt"),
        "A.Manager belongs to A.\n",
    )
    .unwrap();
    std::fs::write(
        root.join("docs/guide.md"),
        "Use A.Manager from project A.\n",
    )
    .unwrap();
    RenameFixture { root }
}

fn field_rename_fixture(tag: &str) -> RenameFixture {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "twaco-field-rename-{tag}-{}-{nonce}",
        std::process::id()
    ));
    for directory in ["DataShapes", "Things", "src/P.D"] {
        std::fs::create_dir_all(root.join(directory)).unwrap();
    }
    std::fs::write(
        root.join("twaco.toml"),
        "[[project]]\nname = \"P\"\ncollections = [\"DataShapes\", \"Things\"]\n",
    )
    .unwrap();
    let shape = "<Entities><DataShapes><DataShape name=\"P.D\" projectName=\"P\"><FieldDefinitions><FieldDefinition name=\"Name\" baseType=\"STRING\" ordinal=\"1\" description=\"\"/><FieldDefinition name=\"Period\" baseType=\"STRING\" ordinal=\"2\" description=\"\"/><FieldDefinition name=\"UID\" baseType=\"STRING\" ordinal=\"3\" description=\"\"/></FieldDefinitions></DataShape></DataShapes></Entities>\n";
    std::fs::write(root.join("DataShapes/P.D.xml"), shape).unwrap();
    let fields = twaco::core::datashape::extract(shape.as_bytes()).unwrap();
    std::fs::write(
        root.join("src/P.D/fields.json"),
        twaco::core::datashape::to_sidecar(&fields),
    )
    .unwrap();
    std::fs::write(root.join("Things/P.T.xml"), "<Entities><Things><Thing name=\"P.T\" projectName=\"P\"><ConfigurationTables><ConfigurationTable dataShapeName=\"P.D\" name=\"T\"><DataShape><FieldDefinitions><FieldDefinition name=\"Period\"/></FieldDefinitions></DataShape><Rows><Row><Period><![CDATA[value]]></Period></Row><Row><Period/></Row></Rows></ConfigurationTable></ConfigurationTables></Thing></Things></Entities>\n").unwrap();
    RenameFixture { root }
}

fn service_rename_fixture(tag: &str) -> RenameFixture {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "twaco-service-rename-{tag}-{}-{nonce}",
        std::process::id()
    ));
    for directory in ["ThingShapes", "src/P.Shape/services/Run"] {
        std::fs::create_dir_all(root.join(directory)).unwrap();
    }
    std::fs::write(
        root.join("twaco.toml"),
        "[[project]]\nname = \"P\"\ncollections = [\"ThingShapes\"]\n",
    )
    .unwrap();
    std::fs::write(root.join("ThingShapes/P.Shape.xml"), "<Entities><ThingShapes><ThingShape name=\"P.Shape\" projectName=\"P\"><ServiceDefinitions><ServiceDefinition name=\"Run\"/></ServiceDefinitions><ServiceImplementations><ServiceImplementation name=\"Run\" handlerName=\"Script\"><ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row><code><![CDATA[me.Run();]]></code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations></ThingShape></ThingShapes></Entities>\n").unwrap();
    std::fs::write(
        root.join("src/P.Shape/services/Run/definition.xml"),
        "<ServiceDefinition name=\"Run\"/>\n",
    )
    .unwrap();
    std::fs::write(root.join("src/P.Shape/services/Run/script.js"), "me.Run();").unwrap();
    RenameFixture { root }
}

fn table_rename_fixture(tag: &str) -> RenameFixture {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "twaco-table-rename-{tag}-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(root.join("Things")).unwrap();
    std::fs::write(
        root.join("twaco.toml"),
        "[[project]]\nname = \"P\"\ncollections = [\"Things\"]\n",
    )
    .unwrap();
    std::fs::write(root.join("Things/P.T.xml"), "<Entities><Things><Thing name=\"P.T\" projectName=\"P\" thingTemplate=\"GenericThing\"><ConfigurationTableDefinitions><ConfigurationTableDefinition dataShapeName=\"P.Limits_CT\" name=\"Limits_CT\"/></ConfigurationTableDefinitions><ConfigurationTables><ConfigurationTable dataShapeName=\"P.Limits_CT\" name=\"Limits_CT\"><DataShape><FieldDefinitions><FieldDefinition name=\"Value\"/></FieldDefinitions></DataShape><Rows><Row><Value>kept</Value></Row></Rows></ConfigurationTable></ConfigurationTables></Thing></Things></Entities>\n").unwrap();
    RenameFixture { root }
}

fn rename_twaco(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_twaco"))
        .args(args)
        .current_dir(root)
        .env_clear()
        .output()
        .unwrap()
}

fn tree_hash(root: &Path, include_ledger: bool) -> [u8; 32] {
    fn visit(root: &Path, directory: &Path, paths: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, paths);
            } else {
                paths.push(path.strip_prefix(root).unwrap().to_path_buf());
            }
        }
    }
    let mut paths = Vec::new();
    visit(root, root, &mut paths);
    paths.sort();
    let mut hash = Sha256::new();
    for relative in paths {
        if !include_ledger && relative.starts_with(".twaco") {
            continue;
        }
        hash.update(relative.to_string_lossy().as_bytes());
        hash.update([0]);
        hash.update(std::fs::read(root.join(&relative)).unwrap());
        hash.update([0]);
    }
    hash.finalize().into()
}

#[test]
fn rename_plan_is_read_only_and_reports_plain_json_and_detail() {
    let fixture = rename_fixture("plan");
    let before = tree_hash(&fixture.root, true);
    let plan = rename_twaco(&fixture.root, &["rename", "prefix", "A", "B"]);
    assert!(
        plan.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&plan.stderr)
    );
    let stdout = String::from_utf8_lossy(&plan.stdout);
    assert!(
        stdout.contains("rename prefix A -> B: a plan, nothing was written"),
        "{stdout}"
    );
    assert!(
        stdout.contains("moves") && stdout.contains("entity files") && stdout.contains("elsewhere"),
        "{stdout}"
    );
    assert!(
        stdout.contains("not changed unless --text")
            && stdout.contains("Not carried over by a rename"),
        "{stdout}"
    );
    assert_eq!(
        tree_hash(&fixture.root, true),
        before,
        "a plan must leave the whole tree byte-identical"
    );

    let json = rename_twaco(&fixture.root, &["rename", "prefix", "A", "B", "--json"]);
    assert!(json.status.success());
    let value: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    for field in [
        "spec",
        "moves",
        "counts",
        "review",
        "outside_applied",
        "applied",
        "verification",
        "follow_up",
        "skipped",
    ] {
        assert!(value.get(field).is_some(), "missing {field}: {value}");
    }
    assert_eq!(value["applied"], false);
    assert!(value["verification"].is_null());
    assert!(value["counts"]["entity"]["files"].as_u64().unwrap() > 0);

    let detail = rename_twaco(&fixture.root, &["rename", "prefix", "A", "B", "--detail"]);
    let stdout = String::from_utf8_lossy(&detail.stdout);
    assert!(
        stdout.contains("Things\\A.Manager.xml:1") || stdout.contains("Things/A.Manager.xml:1"),
        "{stdout}"
    );
}

#[test]
fn rename_apply_moves_files_records_and_verifies_then_round_trips() {
    let fixture = rename_fixture("apply");
    let before = tree_hash(&fixture.root, false);
    let applied = rename_twaco(&fixture.root, &["rename", "prefix", "A", "B", "--apply"]);
    assert!(
        applied.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&applied.stdout),
        String::from_utf8_lossy(&applied.stderr)
    );
    let stdout = String::from_utf8_lossy(&applied.stdout);
    assert!(
        stdout.contains("applied")
            && stdout.contains("sync: in step")
            && stdout.contains("check: all gates pass"),
        "{stdout}"
    );
    assert!(!fixture.root.join("Things/A.Manager.xml").exists());
    assert!(fixture.root.join("Things/B.Manager.xml").exists());
    assert!(!fixture.root.join("src/A.Manager").exists());
    assert!(fixture.root.join("src/B.Manager").exists());
    let ledger: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.root.join(".twaco/renames.json")).unwrap())
            .unwrap();
    assert_eq!(ledger.as_array().unwrap().len(), 1);
    assert!(rename_twaco(&fixture.root, &["check"]).status.success());
    let sync = rename_twaco(&fixture.root, &["sync", "--all", "--check"]);
    assert!(
        sync.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&sync.stdout),
        String::from_utf8_lossy(&sync.stderr)
    );
    assert!(String::from_utf8_lossy(&sync.stdout).contains("already in sync"));

    let reversed = rename_twaco(&fixture.root, &["rename", "prefix", "B", "A", "--apply"]);
    assert!(
        reversed.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&reversed.stdout),
        String::from_utf8_lossy(&reversed.stderr)
    );
    assert_eq!(
        tree_hash(&fixture.root, false),
        before,
        "A -> B -> A must restore every source byte"
    );
    let ledger: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.root.join(".twaco/renames.json")).unwrap())
            .unwrap();
    assert_eq!(ledger.as_array().unwrap().len(), 2);
}

#[test]
fn rename_entity_leaves_a_dotted_child_alone_and_text_is_opt_in() {
    let fixture = rename_fixture("entity");
    let child = std::fs::read(fixture.root.join("Things/A.Manager.Child.xml")).unwrap();
    let docs = std::fs::read(fixture.root.join("docs/guide.md")).unwrap();
    let applied = rename_twaco(
        &fixture.root,
        &["rename", "entity", "A.Manager", "A.Director", "--apply"],
    );
    assert!(
        applied.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&applied.stderr)
    );
    assert_eq!(
        std::fs::read(fixture.root.join("Things/A.Manager.Child.xml")).unwrap(),
        child
    );
    assert_eq!(
        std::fs::read(fixture.root.join("docs/guide.md")).unwrap(),
        docs
    );

    let text_fixture = rename_fixture("text");
    let applied = rename_twaco(
        &text_fixture.root,
        &[
            "rename",
            "entity",
            "A.Manager",
            "A.Director",
            "--text",
            "--apply",
        ],
    );
    assert!(
        applied.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&applied.stderr)
    );
    assert!(
        std::fs::read_to_string(text_fixture.root.join("docs/guide.md"))
            .unwrap()
            .contains("A.Director")
    );
    assert!(
        String::from_utf8_lossy(&applied.stdout).contains("elsewhere")
            && String::from_utf8_lossy(&applied.stdout).contains("changed")
    );
}

#[test]
fn rename_refusals_and_unknown_flag_exit_failed() {
    let fixture = rename_fixture("refuse");
    for args in [
        &["rename", "entity", "Missing", "B"][..],
        &["rename", "entity", "A.Manager", "Taken"][..],
        &["rename", "entity", "A.Manager", "bad/name"][..],
        &["rename", "prefix", "A", "A.New"][..],
        &["rename", "entity", "A.Manager", "B", "--wat"][..],
    ] {
        let output = rename_twaco(&fixture.root, args);
        assert_eq!(
            output.status.code(),
            Some(2),
            "args={args:?}\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("twaco:"));
    }

    std::fs::write(fixture.root.join("notes.md"), "mixed\r\nlines\n").unwrap();
    let blocked = rename_twaco(&fixture.root, &["rename", "entity", "A.Manager", "B"]);
    assert_eq!(blocked.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&blocked.stderr);
    assert!(
        stderr.contains("line endings") && stderr.contains("--skip-checks"),
        "{stderr}"
    );
    let skipped = rename_twaco(
        &fixture.root,
        &["rename", "entity", "A.Manager", "B", "--skip-checks"],
    );
    assert!(
        skipped.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&skipped.stderr)
    );
}

#[test]
fn rename_field_plans_applies_verifies_and_reports_scope() {
    let fixture = field_rename_fixture("cli");
    let before = tree_hash(&fixture.root, true);
    let plan = rename_twaco(
        &fixture.root,
        &["rename", "field", "P.D", "Period", "PeriodKey"],
    );
    assert!(
        plan.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&plan.stderr)
    );
    let stdout = String::from_utf8_lossy(&plan.stdout);
    assert!(
        stdout.contains("rename field P.D: Period -> PeriodKey") && stdout.contains("1 tables"),
        "{stdout}"
    );
    assert_eq!(tree_hash(&fixture.root, true), before);

    let json = rename_twaco(
        &fixture.root,
        &["rename", "field", "P.D", "Period", "PeriodKey", "--json"],
    );
    let value: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(value["spec"]["scope"], "P.D");
    assert_eq!(value["applied"], false);

    let applied = rename_twaco(
        &fixture.root,
        &["rename", "field", "P.D", "Period", "PeriodKey", "--apply"],
    );
    assert!(
        applied.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&applied.stdout),
        String::from_utf8_lossy(&applied.stderr)
    );
    assert!(std::fs::read_to_string(fixture.root.join("Things/P.T.xml"))
        .unwrap()
        .contains("<PeriodKey>"));
    assert!(rename_twaco(&fixture.root, &["check"]).status.success());
    let sync = rename_twaco(&fixture.root, &["sync", "--all", "--check"]);
    assert!(
        sync.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&sync.stdout),
        String::from_utf8_lossy(&sync.stderr)
    );

    let text = rename_twaco(
        &fixture.root,
        &["rename", "field", "P.D", "PeriodKey", "Period", "--text"],
    );
    assert_eq!(text.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&text.stderr).contains("a field rename has no text pass"));
}

#[test]
fn rename_service_plans_applies_checks_json_and_refuses_text() {
    let fixture = service_rename_fixture("cli");
    let before = tree_hash(&fixture.root, true);
    let plan = rename_twaco(
        &fixture.root,
        &["rename", "service", "P.Shape", "Run", "Execute"],
    );
    assert!(
        plan.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let stdout = String::from_utf8_lossy(&plan.stdout);
    assert!(
        stdout.contains("rename service P.Shape: Run -> Execute") && stdout.contains("mashups"),
        "{stdout}"
    );
    assert_eq!(tree_hash(&fixture.root, true), before);

    let json = rename_twaco(
        &fixture.root,
        &["rename", "service", "P.Shape", "Run", "Execute", "--json"],
    );
    let value: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(value["spec"]["kind"], "service");
    assert_eq!(value["spec"]["scope"], "P.Shape");
    assert_eq!(value["applied"], false);

    let applied = rename_twaco(
        &fixture.root,
        &["rename", "service", "P.Shape", "Run", "Execute", "--apply"],
    );
    assert!(
        applied.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&applied.stdout),
        String::from_utf8_lossy(&applied.stderr)
    );
    assert!(fixture.root.join("src/P.Shape/services/Execute").is_dir());
    assert!(rename_twaco(&fixture.root, &["check"]).status.success());
    let sync = rename_twaco(&fixture.root, &["sync", "--all", "--check"]);
    assert!(
        sync.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&sync.stdout),
        String::from_utf8_lossy(&sync.stderr)
    );

    let text = rename_twaco(
        &fixture.root,
        &["rename", "service", "P.Shape", "Execute", "Run", "--text"],
    );
    assert_eq!(text.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&text.stderr).contains("a service rename has no text pass"));
}

#[test]
fn rename_param_plans_applies_checks_round_trips_and_refuses_text() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("twaco-param-rename-{}-{nonce}", std::process::id()));
    std::fs::create_dir_all(root.join("ThingShapes")).unwrap();
    std::fs::create_dir_all(root.join("src/P.Shape/services/Run")).unwrap();
    std::fs::write(
        root.join("twaco.toml"),
        "[[project]]\nname = \"P\"\ncollections = [\"ThingShapes\"]\n",
    )
    .unwrap();
    let definition = "<ServiceDefinition name=\"Run\"><ParameterDefinitions><FieldDefinition baseType=\"NUMBER\" name=\"cardUid\" ordinal=\"1\"/></ParameterDefinitions></ServiceDefinition>";
    let script = "const n = Number(cardUid);\nlogger.warn(\"x\", me.name, cardUid);\nreturn n;";
    std::fs::write(
        root.join("ThingShapes/P.Shape.xml"),
        format!("<Entities><ThingShapes><ThingShape name=\"P.Shape\" projectName=\"P\"><ServiceDefinitions>{definition}</ServiceDefinitions><ServiceImplementations><ServiceImplementation name=\"Run\" handlerName=\"Script\"><ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row><code><![CDATA[{script}]]></code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations></ThingShape></ThingShapes></Entities>\n"),
    )
    .unwrap();
    std::fs::write(
        root.join("src/P.Shape/services/Run/definition.xml"),
        format!("{definition}\n"),
    )
    .unwrap();
    std::fs::write(root.join("src/P.Shape/services/Run/script.js"), script).unwrap();
    let fixture = RenameFixture { root };

    let before = tree_hash(&fixture.root, true);
    let plan = rename_twaco(
        &fixture.root,
        &["rename", "param", "P.Shape", "Run", "cardUid", "cardId"],
    );
    assert!(
        plan.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let stdout = String::from_utf8_lossy(&plan.stdout);
    assert!(
        stdout.contains("rename param P.Shape.Run: cardUid -> cardId"),
        "{stdout}"
    );
    assert_eq!(
        tree_hash(&fixture.root, true),
        before,
        "a plan writes nothing"
    );

    // A new name the script already uses would capture it (`n = Number(n)`), and a reserved word
    // or a name every script has would shadow or not compile: both are refused, nothing written.
    let capture = rename_twaco(
        &fixture.root,
        &["rename", "param", "P.Shape", "Run", "cardUid", "n"],
    );
    assert_eq!(capture.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&capture.stderr).contains("already used"),
        "{}",
        String::from_utf8_lossy(&capture.stderr)
    );
    let reserved = rename_twaco(
        &fixture.root,
        &["rename", "param", "P.Shape", "Run", "cardUid", "me"],
    );
    assert_eq!(reserved.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&reserved.stderr).contains("reserved"),
        "{}",
        String::from_utf8_lossy(&reserved.stderr)
    );
    assert_eq!(tree_hash(&fixture.root, true), before);

    let json = rename_twaco(
        &fixture.root,
        &[
            "rename", "param", "P.Shape", "Run", "cardUid", "cardId", "--json",
        ],
    );
    let value: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(value["spec"]["kind"], "param");
    assert_eq!(value["applied"], false);

    let applied = rename_twaco(
        &fixture.root,
        &[
            "rename", "param", "P.Shape", "Run", "cardUid", "cardId", "--apply",
        ],
    );
    assert!(
        applied.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&applied.stdout),
        String::from_utf8_lossy(&applied.stderr)
    );
    let entity = std::fs::read_to_string(fixture.root.join("ThingShapes/P.Shape.xml")).unwrap();
    assert!(
        entity.contains("name=\"cardId\"") && !entity.contains("cardUid"),
        "{entity}"
    );
    assert!(
        entity.contains("Number(cardId)") && entity.contains("me.name, cardId"),
        "{entity}"
    );
    assert!(rename_twaco(&fixture.root, &["check"]).status.success());
    let sync = rename_twaco(&fixture.root, &["sync", "--all", "--check"]);
    assert!(
        sync.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&sync.stdout),
        String::from_utf8_lossy(&sync.stderr)
    );

    // And back: every file is byte-identical again (the ledger is the only addition).
    let back = rename_twaco(
        &fixture.root,
        &[
            "rename", "param", "P.Shape", "Run", "cardId", "cardUid", "--apply",
        ],
    );
    assert!(
        back.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&back.stderr)
    );
    assert_eq!(tree_hash(&fixture.root, false), before);

    let text = rename_twaco(
        &fixture.root,
        &[
            "rename", "param", "P.Shape", "Run", "cardUid", "cardId", "--text",
        ],
    );
    assert_eq!(text.status.code(), Some(2));
}

#[test]
fn rename_table_plans_applies_checks_json_and_refuses_text() {
    let fixture = table_rename_fixture("cli");
    let before = tree_hash(&fixture.root, true);
    let plan = rename_twaco(
        &fixture.root,
        &["rename", "table", "P.T", "Limits_CT", "Bounds_CT"],
    );
    assert!(
        plan.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let stdout = String::from_utf8_lossy(&plan.stdout);
    assert!(
        stdout.contains("rename table P.T: Limits_CT -> Bounds_CT")
            && stdout.contains("2 tables")
            && stdout.contains("scripts"),
        "{stdout}"
    );
    assert_eq!(tree_hash(&fixture.root, true), before);

    let json = rename_twaco(
        &fixture.root,
        &["rename", "table", "P.T", "Limits_CT", "Bounds_CT", "--json"],
    );
    let value: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(value["spec"]["kind"], "table");
    assert_eq!(value["spec"]["scope"], "P.T");
    assert_eq!(value["tables"], 2);
    assert_eq!(value["applied"], false);

    let applied = rename_twaco(
        &fixture.root,
        &[
            "rename",
            "table",
            "P.T",
            "Limits_CT",
            "Bounds_CT",
            "--apply",
        ],
    );
    assert!(
        applied.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&applied.stdout),
        String::from_utf8_lossy(&applied.stderr)
    );
    let entity = std::fs::read_to_string(fixture.root.join("Things/P.T.xml")).unwrap();
    assert_eq!(entity.matches("name=\"Bounds_CT\"").count(), 2);
    assert!(
        entity.contains("dataShapeName=\"P.Limits_CT\"") && entity.contains("<Value>kept</Value>")
    );
    assert!(rename_twaco(&fixture.root, &["check"]).status.success());
    let sync = rename_twaco(&fixture.root, &["sync", "--all", "--check"]);
    assert!(
        sync.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&sync.stdout),
        String::from_utf8_lossy(&sync.stderr)
    );

    let text = rename_twaco(
        &fixture.root,
        &["rename", "table", "P.T", "Bounds_CT", "Limits_CT", "--text"],
    );
    assert_eq!(text.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&text.stderr).contains("a table rename has no text pass"));
}
