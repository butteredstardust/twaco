use super::super::*;
use super::entity::entity_delete_force_deprecation;
use super::spec::{self, COMMANDS};

/// What `twaco` with no arguments lists for one command.
fn usage_of(command: &str) -> String {
    spec::command(command)
        .map(|command| command.text.to_string())
        .unwrap_or_default()
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
        "permissions",
    ];
    let mut paths = std::collections::BTreeSet::new();
    let blocks: String = COMMANDS
        .iter()
        .map(|c| {
            format!(
                "{}
",
                c.text
            )
        })
        .collect();
    for line in blocks
        .lines()
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
            "{} is not a command path in the listing",
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
            !names(&own, flag) && !(shared && names(&spec::listing(), flag))
        })
        .map(|flag| format!("{command} {flag}"))
        .collect()
}

#[test]
fn usage_names_every_flag_each_command_accepts() {
    let mut absent = Vec::new();
    for command in COMMANDS {
        assert!(
            !command.text.is_empty(),
            "{} has no usage block",
            command.path
        );
        absent.extend(missing(command.path, command.flags));
    }
    assert!(
        absent.is_empty(),
        "flags accepted but not in the command's usage: {absent:?}"
    );
}

#[test]
fn every_command_has_a_handler_or_a_route_of_its_own() {
    // clap knows a command only from COMMANDS; a parse of each finds it again.
    for command in COMMANDS {
        let mut words = vec!["twaco".to_string()];
        words.extend(command.path.split(' ').map(str::to_string));
        let matches = spec::tree()
            .try_get_matches_from(&words)
            .unwrap_or_else(|e| panic!("{}: {e}", command.path));
        let (found, matched) = spec::matched(&matches);
        assert_eq!(found.path, command.path);
        spec::args_of(found, matched).unwrap();
    }
    spec::tree().debug_assert();
}

#[test]
fn every_flag_a_command_lists_is_declared_once() {
    let mut seen = std::collections::BTreeSet::new();
    for flag in spec::FLAGS {
        assert!(seen.insert(flag.name), "{} is declared twice", flag.name);
    }
    for command in COMMANDS {
        let mut own = std::collections::BTreeSet::new();
        for name in command.flags {
            assert!(
                seen.contains(name),
                "{}: {name} is not declared",
                command.path
            );
            assert!(own.insert(name), "{}: {name} is listed twice", command.path);
        }
    }
}

#[test]
fn a_flag_typo_is_refused_with_a_suggestion_and_exit_2() {
    let error = spec::tree()
        .try_get_matches_from(["twaco", "sync", "--cehck"])
        .unwrap_err();
    assert_eq!(error.exit_code(), 2);
    assert!(error.to_string().contains("--check"), "{error}");
    let error = spec::tree()
        .try_get_matches_from(["twaco", "fmt", "stray"])
        .unwrap_err();
    assert_eq!(
        error.exit_code(),
        2,
        "a command without operands refuses one"
    );
}

#[test]
fn values_reach_the_fields_the_commands_read() {
    let parse = |words: &[&str]| {
        let mut all = vec!["twaco"];
        all.extend(words);
        let matches = spec::tree()
            .try_get_matches_from(all)
            .map_err(|e| e.to_string())?;
        let (command, matched) = spec::matched(&matches);
        spec::args_of(command, matched)
    };
    let deploy = parse(&[
        "deploy",
        "--only",
        "A",
        "--only",
        "B",
        "--only-projects",
        "P, Q",
        "--profile",
        "x",
    ])
    .unwrap();
    assert_eq!(deploy.only, ["A", "B"]);
    assert_eq!(deploy.only_projects, ["P", "Q"]);
    assert_eq!(deploy.profile.as_deref(), Some("x"));
    let logs = parse(&["logs", "ScriptLog", "--since", "10m", "--json"]).unwrap();
    assert_eq!(logs.names, ["ScriptLog"]);
    assert_eq!(logs.values.get("--since").map(String::as_str), Some("10m"));
    assert!(logs.has("--json"));
    let query = parse(&["db", "query", "-q", "select 1", "--timeout", "30"]).unwrap();
    assert_eq!(query.values.get("-q").map(String::as_str), Some("select 1"));
    assert_eq!(query.timeout, Some(Duration::from_secs(30)));
    assert!(parse(&["call", "T", "S", "--timeout", "0"]).is_err());
    assert!(parse(&["deploy", "--only-projects", " , "]).is_err());
    assert!(parse(&["sync", "--all", "Acme.T"]).is_err());
    // A value may begin with a dash, as a log filter can.
    let grep = parse(&["logs", "ScriptLog", "--grep", "-x"]).unwrap();
    assert_eq!(grep.values.get("--grep").map(String::as_str), Some("-x"));
    // A negative number is an operand; a mistyped flag after operands is still refused.
    let call = parse(&["call", "T", "S", "-5"]).unwrap();
    assert_eq!(call.names, ["T", "S", "-5"]);
    assert!(parse(&["sync", "Acme.T", "--cehck"]).is_err());
    // A file named with a leading dash goes after `--`.
    let file = parse(&["db", "run", "--", "-migration.sql"]).unwrap();
    assert_eq!(file.names, ["-migration.sql"]);
    // A switch said twice is said once; a value given twice is refused, as before.
    assert!(parse(&["deploy", "--force", "--force"])
        .unwrap()
        .has("--force"));
    assert!(parse(&["deploy", "--profile", "a", "--profile", "b"]).is_err());
    // `--flag=value` works too.
    let profile = parse(&["doctor", "--profile=prod"]).unwrap();
    assert_eq!(profile.profile.as_deref(), Some("prod"));
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

/// `documentation/COMMANDS.md` is the command table written out. `TWACO_BLESS=1` rewrites it.
#[test]
fn commands_md_is_the_command_table() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/documentation/COMMANDS.md");
    let generated = spec::commands_md();
    if std::env::var_os("TWACO_BLESS").is_some() {
        std::fs::write(path, &generated).unwrap();
        return;
    }
    let Ok(written) = std::fs::read_to_string(path) else {
        eprintln!("skipping: {path} is not packaged");
        return;
    };
    assert!(
        written.replace("\r\n", "\n") == generated,
        "documentation/COMMANDS.md is not the command table; run TWACO_BLESS=1 cargo test --bin twaco commands_md"
    );
}
