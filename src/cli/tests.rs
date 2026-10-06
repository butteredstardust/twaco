use super::super::*;
use super::entity::entity_delete_force_deprecation;
use super::info::{GUIDE_FLAGS, HELP_FLAGS, JAVADOC_FLAGS};
use super::usage::USAGE;

/// The usage text of one command: every block whose first line names it, with the lines
/// indented under it. `scripts/commands_doc.py` groups the text the same way.
fn usage_of(command: &str) -> String {
    let mut text = String::new();
    let mut current = false;
    for line in USAGE.lines().skip(2) {
        if line.starts_with("  ") && !line.starts_with("    ") {
            let words: Vec<&str> = line.split_whitespace().collect();
            let key = if matches!(
                words[0],
                "entity" | "rename" | "db" | "datatable" | "move" | "copy" | "new"
            ) {
                format!("{} {}", words[0], words[1])
            } else {
                words[0].to_string()
            };
            current = key == command;
        }
        if current {
            text.push_str(line);
            text.push('\n');
        }
    }
    text
}

#[derive(Debug)]
struct MutationRow {
    name: String,
    default: String,
    applied: String,
}

/// Read one of MUTATION_CLASSES.md's tables without adding a Markdown parser to the binary.
fn mutation_rows(document: &str, heading: &str) -> Vec<MutationRow> {
    let mut in_table = false;
    let mut rows = Vec::new();
    for line in document.lines() {
        if line == heading {
            in_table = true;
            continue;
        }
        if in_table && line.starts_with("## ") {
            break;
        }
        if !in_table || !line.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        if cells.len() != 8 || !cells[1].starts_with('`') || !cells[1].ends_with('`') {
            continue;
        }
        rows.push(MutationRow {
            name: cells[1].trim_matches('`').to_string(),
            default: cells[2].to_string(),
            applied: cells[3].to_string(),
        });
    }
    rows
}

/// Command paths as the usage headings name them.  A heading may describe several forms;
/// its shared path is enough to reject a table row for a command absent from usage.
fn usage_paths() -> std::collections::BTreeSet<String> {
    let two_words = [
        "entity",
        "rename",
        "db",
        "datatable",
        "move",
        "copy",
        "new",
        "export",
        "package",
        "import",
        "ext",
        "repo",
        "help",
        "javadoc",
    ];
    let mut paths = std::collections::BTreeSet::new();
    for line in USAGE
        .lines()
        .skip(2)
        .filter(|line| line.starts_with("  ") && !line.starts_with("    "))
    {
        let words: Vec<&str> = line.split_whitespace().collect();
        let Some(first) = words.first() else { continue };
        if first.starts_with('-') {
            continue;
        }
        let path = if *first == "logs" && words.get(1) == Some(&"level") {
            "logs level".to_string()
        } else if two_words.contains(first) {
            words
                .get(1)
                .filter(|word| !word.starts_with(['[', '<']))
                .map(|word| format!("{first} {word}"))
                .unwrap_or_else(|| (*first).to_string())
        } else {
            (*first).to_string()
        };
        paths.insert(path);
    }
    paths
}

fn mutation_document() -> Option<String> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/documentation/MUTATION_CLASSES.md"
    );
    match std::fs::read_to_string(path) {
        Ok(document) => Some(document),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skipping mutation-class documentation checks: {path} is not packaged");
            None
        }
        Err(error) => panic!("cannot read {path}: {error}"),
    }
}

#[test]
fn mutation_classes_name_every_cli_command_and_known_class() {
    let Some(document) = mutation_document() else {
        return;
    };
    let rows = mutation_rows(&document, "## CLI commands");
    let paths = usage_paths();
    let classes = [
        "read-only",
        "single-file atomic",
        "multi-file atomic",
        "best-effort batch",
        "server-partial",
    ];
    let mut names = std::collections::BTreeSet::new();
    for row in &rows {
        assert!(
            names.insert(&row.name),
            "CLI mutation class is duplicated: {}",
            row.name
        );
        assert!(
            classes.contains(&row.default.as_str()),
            "{} has invalid Default class {:?}",
            row.name,
            row.default
        );
        assert!(
            classes.contains(&row.applied.as_str()),
            "{} has invalid Applied class {:?}",
            row.name,
            row.applied
        );
        let path = row.name.split(" --").next().unwrap();
        assert!(
            paths.contains(path),
            "{} is not a command path in USAGE",
            row.name
        );
    }
    for path in &paths {
        assert!(
            rows.iter()
                .any(|row| row.name == *path || row.name.starts_with(&format!("{path} --"))),
            "{path} has no CLI mutation-class row"
        );
    }
}

#[test]
fn mutation_classes_hold_cli_plans_and_workspace_writes() {
    let Some(document) = mutation_document() else {
        return;
    };
    let rows = mutation_rows(&document, "## CLI commands");
    for row in &rows {
        let command = row.name.split(" --").next().unwrap();
        let args: Vec<String> = command.split_whitespace().map(str::to_string).collect();
        if let Ok((_, _, flags)) = route(&args) {
            // `export` has one routed flag list, but only its source-control form uses
            // --apply; entity, collection and project write their requested local output.
            if flags.contains(&"--apply")
                && !matches!(
                    command,
                    "export entity" | "export collection" | "export project"
                )
            {
                assert_eq!(
                    row.default, "read-only",
                    "{command} plans by default, so its Default class must be read-only"
                );
            }
        }
    }
    for (command, args, generic_lock) in [
        ("bundle", vec![], false),
        ("deploy", vec!["--apply"], false),
        ("entity status", vec!["--record"], false),
        ("repo pull", vec!["pull", "--apply"], false),
    ] {
        let route_args: Vec<String> = command.split_whitespace().map(str::to_string).collect();
        let (route_name, _, flags) = route(&route_args).unwrap();
        let parsed = Args::parse(
            &args.into_iter().map(str::to_string).collect::<Vec<_>>(),
            flags,
        )
        .unwrap();
        assert_eq!(
            writes_workspace(route_name, &parsed),
            generic_lock,
            "{command} must {} the generic workspace lock",
            if generic_lock {
                "take"
            } else {
                "leave to its executor"
            }
        );
        let row = rows
            .iter()
            .find(|row| row.name == command)
            .unwrap_or_else(|| panic!("{command} has no CLI mutation-class row"));
        assert_ne!(
            row.applied, "read-only",
            "{command} writes the workspace, so its Applied class cannot be read-only"
        );
    }
}

#[test]
fn mutation_classes_name_every_mcp_tool_and_its_dry_runs() {
    let Some(document) = mutation_document() else {
        return;
    };
    let rows = mutation_rows(&document, "## MCP tools");
    let tools = twaco::mcp::tool_definitions();
    let known: std::collections::BTreeSet<&str> = tools
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    let classes = [
        "read-only",
        "single-file atomic",
        "multi-file atomic",
        "best-effort batch",
        "server-partial",
    ];
    let mut names = std::collections::BTreeSet::new();
    for row in &rows {
        assert!(
            names.insert(&row.name),
            "MCP mutation class is duplicated: {}",
            row.name
        );
        assert!(
            known.contains(row.name.as_str()),
            "{} is not an MCP tool",
            row.name
        );
        assert!(
            classes.contains(&row.default.as_str()),
            "{} has invalid Default class {:?}",
            row.name,
            row.default
        );
        assert!(
            classes.contains(&row.applied.as_str()),
            "{} has invalid Applied class {:?}",
            row.name,
            row.applied
        );
    }
    for tool in tools {
        let name = tool["name"].as_str().unwrap();
        let row = rows
            .iter()
            .find(|row| row.name == name)
            .unwrap_or_else(|| panic!("{name} has no MCP mutation-class row"));
        let dry_run = tool
            .pointer("/inputSchema/properties/dry_run/default")
            .and_then(serde_json::Value::as_bool);
        if dry_run == Some(true) {
            assert_eq!(
                row.default, "read-only",
                "{name} has dry_run defaulting to true, so its Default class must be read-only"
            );
        }
    }
}

/// Whether `text` names `flag` as a whole word, so `--only` is not found inside
/// `--only-projects`.
fn names(text: &str, flag: &str) -> bool {
    text.match_indices(flag).any(|(at, _)| {
        !text[at + flag.len()..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '-')
    })
}

/// `--profile` and `--project` are described once, for every command that takes them.
fn missing(command: &str, known: &[&str]) -> Vec<String> {
    let own = usage_of(command);
    known
        .iter()
        .filter(|flag| {
            let shared = matches!(**flag, "--profile" | "--project");
            !names(&own, flag) && !(shared && names(USAGE, flag))
        })
        .map(|flag| format!("{command} {flag}"))
        .collect()
}

#[test]
fn usage_names_every_flag_each_command_accepts() {
    let commands = [
        "projects",
        "types",
        "extract",
        "sync",
        "fmt",
        "check",
        "bundle",
        "deploy",
        "call",
        "ext",
        "settings",
        "catalog",
        "impact",
        "unused",
        "docs",
        "package",
        "import",
        "export",
        "repo",
        "logs",
        "adopt",
        "rename entity",
        "rename prefix",
        "rename field",
        "rename service",
        "rename param",
        "rename table",
        "rename property",
        "move service",
        "move property",
        "copy service",
        "copy property",
        "retemplate",
        "new building-block",
        "config-table",
        "entity get",
        "entity push",
        "entity delete",
        "entity carry",
        "entity restore",
        "entity status",
        "db run",
        "db query",
        "db clean",
        "datatable copy",
    ];
    let mut absent = Vec::new();
    for command in commands {
        let args: Vec<String> = command.split(' ').map(str::to_string).collect();
        let (_, _, known) = route(&args).unwrap_or_else(|why| panic!("{command}: {why}"));
        assert!(
            !usage_of(command).is_empty(),
            "{command} has no usage block"
        );
        absent.extend(missing(command, known));
    }
    for (command, known) in [
        ("help", HELP_FLAGS),
        ("guide", GUIDE_FLAGS),
        ("javadoc", JAVADOC_FLAGS),
    ] {
        absent.extend(missing(command, known));
    }
    assert!(
        absent.is_empty(),
        "flags accepted but not in the command's usage: {absent:?}"
    );
}

#[test]
fn an_unknown_command_has_no_route() {
    assert!(route(&["nonsense".to_string()]).is_err());
    assert!(route(&["entity".to_string()]).is_err());
    assert!(route(&["entity".to_string(), "frob".to_string()]).is_err());
    assert!(route(&["rename".to_string()]).is_err());
    assert!(route(&["rename".to_string(), "frob".to_string()]).is_err());
}

#[test]
fn entity_delete_takes_no_generic_workspace_lock() {
    let args = Args::parse(
        &["Things/T".to_string(), "--apply".to_string()],
        &["--apply"],
    )
    .unwrap();
    assert!(
        !writes_workspace("entity delete", &args),
        "only a pending rename ledger is a workspace write"
    );
}

#[test]
fn entity_delete_acknowledgement_flags_are_scoped_to_entity_delete() {
    let delete = ["entity".to_string(), "delete".to_string()];
    let (_, _, flags) = route(&delete).unwrap();
    for flag in [
        "--allow-repository-defined",
        "--allow-outside-dependents",
        "--allow-file-repository-data-loss",
    ] {
        assert!(
            Args::parse(&["Things/T".to_string(), flag.to_string()], flags).is_ok(),
            "{flag}"
        );
    }
    let push = ["entity".to_string(), "push".to_string()];
    let (_, _, flags) = route(&push).unwrap();
    for flag in [
        "--allow-repository-defined",
        "--allow-outside-dependents",
        "--allow-file-repository-data-loss",
    ] {
        assert!(
            Args::parse(&["Things/T".to_string(), flag.to_string()], flags).is_err(),
            "{flag}"
        );
    }
}

#[test]
fn entity_push_takes_no_generic_workspace_lock() {
    let args = Args::parse(&["P.T".to_string(), "--apply".to_string()], &["--apply"]).unwrap();
    assert!(
        !writes_workspace("entity push", &args),
        "the command executor takes the lock before it discovers the entity"
    );
}

#[test]
fn facade_commands_take_no_generic_workspace_lock() {
    for (route, args) in [
        (
            "extract",
            Args::parse(&["--all".to_string()], &["--all"]).unwrap(),
        ),
        (
            "types",
            Args::parse(&[], &["--check", "--platform"]).unwrap(),
        ),
        (
            "sync",
            Args::parse(&["--all".to_string()], &["--all"]).unwrap(),
        ),
        ("fmt", Args::parse(&[], &["--check"]).unwrap()),
        (
            "adopt",
            Args::parse(
                &["export.xml".to_string(), "--apply".to_string()],
                &["--apply"],
            )
            .unwrap(),
        ),
        (
            "rename entity",
            Args::parse(
                &["Old".to_string(), "New".to_string(), "--apply".to_string()],
                &["--apply"],
            )
            .unwrap(),
        ),
        (
            "move service",
            Args::parse(
                &[
                    "From".to_string(),
                    "To".to_string(),
                    "Name".to_string(),
                    "--apply".to_string(),
                ],
                &["--apply"],
            )
            .unwrap(),
        ),
        (
            "copy property",
            Args::parse(
                &[
                    "From".to_string(),
                    "To".to_string(),
                    "Name".to_string(),
                    "--apply".to_string(),
                ],
                &["--apply"],
            )
            .unwrap(),
        ),
        (
            "retemplate",
            Args::parse(&["Thing".to_string(), "--apply".to_string()], &["--apply"]).unwrap(),
        ),
        (
            "new building-block",
            Args::parse(
                &["Acme.Block".to_string(), "--apply".to_string()],
                &["--apply"],
            )
            .unwrap(),
        ),
    ] {
        assert!(
            !writes_workspace(route, &args),
            "the {route} executor takes its own workspace lock"
        );
    }
}

#[test]
fn entity_delete_force_deprecation_text_is_stable() {
    assert_eq!(
            entity_delete_force_deprecation(),
            "--force is deprecated for entity delete; it now means --allow-repository-defined --allow-outside-dependents and never covers FileRepository data loss"
        );
}

#[test]
fn entity_carry_takes_no_generic_workspace_lock() {
    let args = Args::parse(
        &["--renamed".to_string(), "--apply".to_string()],
        &["--apply", "--renamed"],
    )
    .unwrap();
    assert!(
        !writes_workspace("entity carry", &args),
        "the command takes its own lock, only when it marks the ledger"
    );
}

#[test]
fn db_run_apply_takes_no_workspace_lock() {
    let args = Args::parse(
        &["migration.sql".to_string(), "--apply".to_string()],
        &["--apply"],
    )
    .unwrap();
    assert!(!writes_workspace("db run", &args));
}
