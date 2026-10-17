use super::super::*;
use super::command::Typed;
use super::entity::entity_delete_force_deprecation;
use super::spec::{self, GRAMMAR, USAGE};
use clap::{FromArgMatches as _, Subcommand as _};

/// The command a command line names: its path, how many words name it, and the flags it takes.
type Route = (&'static str, usize, Vec<String>);

fn route(args: &[String]) -> Result<Route, String> {
    let two = args.iter().take(2).cloned().collect::<Vec<_>>().join(" ");
    let one = args.first().cloned().unwrap_or_default();
    for (path, words) in [(two, 2), (one, 1)] {
        if let Some(command) = spec::command(&path) {
            return Ok((command.path, words, flags_of(command.path)));
        }
    }
    Err(format!("unknown command `{}`", args.join(" ")))
}

/// The command at `path` in the tree clap parses with, however it was declared.
fn node(tree: &clap::Command, path: &str) -> clap::Command {
    let mut node = tree;
    for word in path.split(' ') {
        node = node
            .find_subcommand(word)
            .unwrap_or_else(|| panic!("{path} is not in the tree"));
    }
    node.clone()
}

/// What a flag argument is called on the command line: `--name`, or `-q`.
fn flag_name(arg: &clap::Arg) -> Option<String> {
    arg.get_long()
        .map(|long| format!("--{long}"))
        .or_else(|| arg.get_short().map(|short| format!("-{short}")))
}

/// The flags the command at `path` takes in the built tree, without the diagnostic ones every
/// command takes.
fn flags_of(path: &str) -> Vec<String> {
    node(&spec::tree(), path)
        .get_arguments()
        .filter(|arg| !arg.is_positional())
        .filter_map(flag_name)
        .filter(|name| !spec::GLOBAL_FLAGS.contains(&name.as_str()))
        .collect()
}

/// Parse a table command's line (the words after `twaco`) as `twaco` does, up to what the
/// command reads.
fn parse(words: &[&str]) -> Result<Args, String> {
    let mut all = vec!["twaco"];
    all.extend(words);
    let matches = spec::tree()
        .try_get_matches_from(all)
        .map_err(|e| e.to_string())?;
    let (path, matched) = spec::matched(&matches);
    spec::args_of(path, matched)
}

/// Parse a derived command's line as `twaco` does, up to what the command reads.
fn parse_typed(words: &[&str]) -> Result<Typed, String> {
    let mut all = vec!["twaco"];
    all.extend(words);
    let matches = spec::tree()
        .try_get_matches_from(all)
        .map_err(|e| e.to_string())?;
    let typed = Typed::from_arg_matches(&matches).map_err(|e| e.to_string())?;
    typed.validate()?;
    Ok(typed)
}

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
        "localization",
        "help",
        "javadoc",
        "permissions",
        "mashup",
        "thing",
        "timer",
        "scheduler",
        "style",
        "media",
        "menu",
        "state",
    ];
    let mut paths = std::collections::BTreeSet::new();
    let blocks: String = USAGE
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
            if flags.iter().any(|flag| flag == "--apply")
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
    for command in ["bundle", "deploy", "entity status", "repo pull"] {
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
fn missing(command: &str, known: &[String]) -> Vec<String> {
    let own = usage_of(command);
    known
        .iter()
        .filter(|flag| {
            let shared = matches!(flag.as_str(), "--profile" | "--project");
            !names(&own, flag) && !(shared && names(&spec::listing(), flag))
        })
        .map(|flag| format!("{command} {flag}"))
        .collect()
}

#[test]
fn usage_names_every_flag_each_command_accepts() {
    let mut absent = Vec::new();
    for command in USAGE {
        assert!(
            !command.text.is_empty(),
            "{} has no usage block",
            command.path
        );
        absent.extend(missing(command.path, &flags_of(command.path)));
    }
    assert!(
        absent.is_empty(),
        "flags accepted but not in the command's usage: {absent:?}"
    );
}

#[test]
fn every_command_has_a_handler_or_a_route_of_its_own() {
    // clap knows a command only from USAGE; a parse of each finds it again.
    for command in USAGE {
        let mut words = vec!["twaco".to_string()];
        words.extend(command.path.split(' ').map(str::to_string));
        let matches = spec::tree()
            .try_get_matches_from(&words)
            .unwrap_or_else(|e| panic!("{}: {e}", command.path));
        let (found, matched) = spec::matched(&matches);
        assert_eq!(found, command.path);
        let first = command.path.split(' ').next().unwrap_or_default();
        if Typed::has_subcommand(first) {
            let typed = Typed::from_arg_matches(&matches).unwrap();
            assert_eq!(typed.path(), command.path);
            typed.validate().unwrap();
            assert!(
                spec::grammar(command.path).is_none(),
                "{} is declared twice",
                command.path
            );
        } else {
            spec::args_of(found, matched).unwrap();
        }
    }
    spec::tree().debug_assert();
}

#[test]
fn every_flag_a_command_lists_is_declared_once() {
    let mut seen = std::collections::BTreeSet::new();
    for flag in spec::FLAGS {
        assert!(seen.insert(flag.name), "{} is declared twice", flag.name);
    }
    for command in GRAMMAR {
        assert!(
            spec::command(command.path).is_some(),
            "{} has a GRAMMAR row but no USAGE row",
            command.path
        );
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

/// Whatever declared a command, its flags are the ones FLAGS declares, taking what FLAGS says
/// and with FLAGS' value names (clap's errors quote them); its operands are `operands`.
#[test]
fn every_flag_in_the_tree_is_declared_with_its_kind() {
    let tree = spec::tree();
    for command in USAGE {
        let node = node(&tree, command.path);
        for arg in node.get_arguments() {
            let value_names: Vec<String> = arg
                .get_value_names()
                .unwrap_or_default()
                .iter()
                .map(ToString::to_string)
                .collect();
            if arg.is_positional() {
                assert_eq!(arg.get_id(), "operands", "{}", command.path);
                assert_eq!(value_names, ["operand"], "{}", command.path);
                continue;
            }
            let name =
                flag_name(arg).unwrap_or_else(|| panic!("{}: an unnamed flag", command.path));
            let declared = match name.as_str() {
                "--log" => spec::Takes::Value("filter"),
                "--log-file" => spec::Takes::Value("path"),
                _ => {
                    assert!(
                        spec::FLAGS.iter().any(|flag| flag.name == name),
                        "{}: {name} is not declared in FLAGS",
                        command.path
                    );
                    spec::flag(&name).takes
                }
            };
            let optional = arg
                .get_num_args()
                .is_some_and(|range| range.min_values() == 0);
            let value = |takes: spec::Takes| match takes {
                spec::Takes::Switch => String::new(),
                spec::Takes::Value(name)
                | spec::Takes::OptionalValue(name)
                | spec::Takes::Repeated(name) => name.to_string(),
            };
            let kind = |takes: spec::Takes| match takes {
                spec::Takes::Switch => "switch",
                spec::Takes::Value(_) => "value",
                spec::Takes::OptionalValue(_) => "optional value",
                spec::Takes::Repeated(_) => "repeated",
            };
            let found = match arg.get_action() {
                clap::ArgAction::SetTrue => "switch",
                clap::ArgAction::Set if optional => "optional value",
                clap::ArgAction::Set => "value",
                clap::ArgAction::Append => "repeated",
                other => panic!("{} {name}: unexpected action {other:?}", command.path),
            };
            assert_eq!(found, kind(declared), "{} {name}", command.path);
            let expected: Vec<String> = Some(value(declared))
                .filter(|name| !name.is_empty())
                .into_iter()
                .collect();
            assert_eq!(value_names, expected, "{} {name}: value name", command.path);
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
    let profile = parse(&["deploy", "--profile=prod"]).unwrap();
    assert_eq!(profile.profile.as_deref(), Some("prod"));
    let Typed::Doctor(doctor) = parse_typed(&["doctor", "--profile=prod"]).unwrap() else {
        panic!("doctor parses as doctor");
    };
    assert_eq!(doctor.profile.name(), "prod");
    let Typed::Doctor(doctor) = parse_typed(&["doctor"]).unwrap() else {
        panic!("doctor parses as doctor");
    };
    assert_eq!(doctor.profile.name(), "default");
    assert!(parse_typed(&["doctor", "--profile", "a", "--profile", "b"]).is_err());
    let Typed::Update(update) = parse_typed(&["update", "--apply", "--apply"]).unwrap() else {
        panic!("update parses as update");
    };
    assert!(update.apply.apply);
    let Typed::Mcp(mcp) = parse_typed(&["mcp", "--tools", "-core"]).unwrap() else {
        panic!("mcp parses as mcp");
    };
    assert_eq!(mcp.tools.as_deref(), Some("-core"));
    // An optional value given bare is followed by the next flag, never takes it as its value.
    let menu = parse(&[
        "new",
        "menu",
        "Acme.Main_MN",
        "--item",
        "Home=Acme.Home_MU",
        "--default",
        "--label",
        "Main",
    ])
    .unwrap();
    assert_eq!(menu.names, ["Acme.Main_MN"]);
    assert!(menu.has("--default") && !menu.values.contains_key("--default"));
    assert_eq!(menu.values.get("--label").map(String::as_str), Some("Main"));
    // It may still be negative.
    let param = parse(&[
        "mashup",
        "param",
        "set",
        "Acme.Home_MU",
        "Count",
        "--default",
        "-1",
    ])
    .unwrap();
    assert_eq!(
        param.values.get("--default").map(String::as_str),
        Some("-1")
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
fn entity_delete_acknowledgement_flags_are_scoped_to_entity_delete() {
    for flag in [
        "--allow-repository-defined",
        "--allow-outside-dependents",
        "--allow-file-repository-data-loss",
    ] {
        assert!(
            parse(&["entity", "delete", "Things/T", flag]).is_ok(),
            "{flag}"
        );
        assert!(
            parse(&["entity", "push", "Things/T", flag]).is_err(),
            "{flag}"
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

/// The grammar clap parses with, written out: for the root, every family and every command, its
/// usage line, its help, its settings, and for each argument how it behaves, read from the built
/// tree and, where clap has no getter, by parsing. Two trees with equal dumps parse alike however
/// their commands were declared, so the dump holds a change of declaration to the grammar it
/// replaces. `TWACO_BLESS=1` rewrites `tests/fixtures/cli_grammar.txt`.
#[test]
fn the_grammar_is_the_dumped_grammar() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/cli_grammar.txt"
    );
    let generated = grammar_dump();
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
        "the command-line grammar is not tests/fixtures/cli_grammar.txt; if the change is meant, run TWACO_BLESS=1 cargo test --bin twaco grammar and review the diff"
    );
}

fn grammar_dump() -> String {
    let mut tree = spec::tree();
    tree.build();
    let mut probe = tree.clone();
    let mut out = String::new();
    dump_node(&mut probe, &tree, &[], &mut out);
    out
}

fn dump_node(probe: &mut clap::Command, node: &clap::Command, words: &[&str], out: &mut String) {
    use std::fmt::Write as _;
    let line = std::iter::once("twaco")
        .chain(words.iter().copied())
        .collect::<Vec<_>>()
        .join(" ");
    writeln!(out, "== {line}").unwrap();
    let usage = node
        .get_overridden_usage()
        .map_or_else(|| "-".to_string(), ToString::to_string);
    writeln!(out, "usage: {usage}").unwrap();
    let settings: Vec<&str> = [
        ("version", node.get_version().is_some()),
        ("disable-version-flag", node.is_disable_version_flag_set()),
        ("disable-help-flag", node.is_disable_help_flag_set()),
        (
            "disable-help-subcommand",
            node.is_disable_help_subcommand_set(),
        ),
        ("subcommand-required", node.is_subcommand_required_set()),
        (
            "arg-required-else-help",
            node.is_arg_required_else_help_set(),
        ),
        ("args-override-self", node.is_args_override_self()),
        (
            "allow-negative-numbers",
            node.is_allow_negative_numbers_set(),
        ),
        (
            "allow-missing-positional",
            node.is_allow_missing_positional_set(),
        ),
        (
            "allow-external-subcommands",
            node.is_allow_external_subcommands_set(),
        ),
        ("trailing-var-arg", node.is_trailing_var_arg_set()),
        ("no-binary-name", node.is_no_binary_name_set()),
        ("multicall", node.is_multicall_set()),
        ("hide", node.is_hide_set()),
        (
            "subcommand-negates-reqs",
            node.is_subcommand_negates_reqs_set(),
        ),
        (
            "subcommand-precedence-over-arg",
            node.is_subcommand_precedence_over_arg_set(),
        ),
        (
            "args-conflicts-with-subcommands",
            node.is_args_conflicts_with_subcommands_set(),
        ),
    ]
    .into_iter()
    .filter_map(|(name, set)| set.then_some(name))
    .collect();
    writeln!(out, "settings: {}", settings.join(" ")).unwrap();
    let aliases: Vec<&str> = node.get_all_aliases().collect();
    if !aliases.is_empty() {
        writeln!(out, "aliases: {}", aliases.join(" ")).unwrap();
    }
    let mut subcommands: Vec<&clap::Command> = node.get_subcommands().collect();
    subcommands.sort_by_key(|sub| sub.get_name());
    let names: Vec<&str> = subcommands.iter().map(|sub| sub.get_name()).collect();
    writeln!(out, "subcommands: {}", names.join(" ")).unwrap();
    if node.get_groups().next().is_some() {
        writeln!(out, "groups: {}", node.get_groups().count()).unwrap();
    }
    let mut args: Vec<&clap::Arg> = node.get_arguments().collect();
    args.sort_by_key(|arg| arg_key(arg));
    for arg in args {
        writeln!(out, "arg {}", arg_key(arg)).unwrap();
        for (field, value) in arg_fields(probe, words, arg) {
            writeln!(out, "    {field}: {value}").unwrap();
        }
    }
    writeln!(out, "help:").unwrap();
    for line in node.clone().render_help().to_string().lines() {
        writeln!(out, "| {line}").unwrap();
    }
    writeln!(out).unwrap();
    for sub in subcommands {
        let mut path = words.to_vec();
        path.push(sub.get_name());
        dump_node(probe, sub, &path, out);
    }
}

/// How the dump names an argument: `--name`, `-q`, or `<id>` for a positional.
fn arg_key(arg: &clap::Arg) -> String {
    if arg.is_positional() {
        format!("<{}>", arg.get_id())
    } else {
        flag_name(arg).unwrap_or_else(|| format!("({})", arg.get_id()))
    }
}

fn arg_fields(
    probe: &mut clap::Command,
    words: &[&str],
    arg: &clap::Arg,
) -> Vec<(&'static str, String)> {
    let yes = |set: bool| if set { "yes" } else { "no" }.to_string();
    let list = |items: Vec<String>| {
        if items.is_empty() {
            "-".to_string()
        } else {
            items.join(" ")
        }
    };
    let mut fields = vec![
        ("action", format!("{:?}", arg.get_action())),
        (
            "value_name",
            list(
                arg.get_value_names()
                    .unwrap_or_default()
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
            ),
        ),
        (
            "num_args",
            arg.get_num_args()
                .map_or_else(|| "-".to_string(), |range| range.to_string()),
        ),
        ("allow_hyphen_values", yes(arg.is_allow_hyphen_values_set())),
        (
            "allow_negative_numbers",
            yes(arg.is_allow_negative_numbers_set()),
        ),
        ("required", yes(arg.is_required_set())),
        (
            "default",
            list(
                arg.get_default_values()
                    .iter()
                    .map(|value| format!("{:?}", value.to_string_lossy()))
                    .collect(),
            ),
        ),
        (
            "index",
            arg.get_index()
                .map_or_else(|| "-".to_string(), |index| index.to_string()),
        ),
    ];
    let mut flags: Vec<&str> = [
        ("global", arg.is_global_set()),
        ("hide", arg.is_hide_set()),
        ("require-equals", arg.is_require_equals_set()),
        ("exclusive", arg.is_exclusive_set()),
        ("last", arg.is_last_set()),
        ("trailing-var-arg", arg.is_trailing_var_arg_set()),
        ("ignore-case", arg.is_ignore_case_set()),
    ]
    .into_iter()
    .filter_map(|(name, set)| set.then_some(name))
    .collect();
    if arg.get_value_delimiter().is_some() {
        flags.push("value-delimiter");
    }
    if !arg.get_possible_values().is_empty() {
        flags.push("possible-values");
    }
    if arg.get_all_aliases().is_some() || arg.get_all_short_aliases().is_some() {
        flags.push("aliases");
    }
    fields.push((
        "other",
        list(flags.iter().map(ToString::to_string).collect()),
    ));
    // What clap has no getter for, asked of a parse: whether the flag may be given twice (a
    // switch overriding itself), and what it holds when given with no value (its default
    // missing value).
    let Some(name) = flag_name(arg).filter(|_| !words.is_empty()) else {
        return fields;
    };
    let takes_value = matches!(
        arg.get_action(),
        clap::ArgAction::Set | clap::ArgAction::Append
    );
    let given = |times: usize| -> Vec<String> {
        let mut argv: Vec<String> = std::iter::once("twaco")
            .chain(words.iter().copied())
            .map(str::to_string)
            .collect();
        for _ in 0..times {
            argv.push(name.clone());
            if takes_value {
                argv.push("v".to_string());
            }
        }
        argv
    };
    if !matches!(
        arg.get_action(),
        clap::ArgAction::Help | clap::ArgAction::Version
    ) {
        let twice = match probe.try_get_matches_from_mut(given(2)) {
            Ok(_) => "accepted".to_string(),
            Err(error) => format!("{:?}", error.kind()),
        };
        fields.push(("given twice", twice));
    }
    if takes_value {
        let mut argv = given(0);
        argv.push(name.clone());
        let bare = match probe.try_get_matches_from_mut(argv) {
            Ok(matches) => {
                let mut leaf = &matches;
                for word in words {
                    leaf = leaf
                        .subcommand_matches(word)
                        .expect("the probe names the leaf");
                }
                let raw: Vec<String> = leaf
                    .get_raw(arg.get_id().as_str())
                    .map(|values| {
                        values
                            .map(|value| format!("{:?}", value.to_string_lossy()))
                            .collect()
                    })
                    .unwrap_or_default();
                format!("holds {}", list(raw))
            }
            Err(error) => format!("{:?}", error.kind()),
        };
        fields.push(("given bare", bare));
    }
    fields
}

/// What a command line says, for [`derived_commands_read_what_their_grammar_rows_read`]: its
/// operands, the switches given and the values given, each value once per time it was given.
type Projection = (Vec<String>, Vec<String>, Vec<(String, String)>);

/// The switches of a derived command that are on.
fn switches(pairs: &[(&str, bool)]) -> Vec<String> {
    pairs
        .iter()
        .filter(|(_, on)| *on)
        .map(|(name, _)| (*name).to_string())
        .collect()
}

/// The values of a derived command that were given.
fn values(pairs: &[(&str, Option<String>)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .filter_map(|(name, value)| value.clone().map(|value| ((*name).to_string(), value)))
        .collect()
}

/// A path value as text.
fn shown(path: Option<&std::path::PathBuf>) -> Option<String> {
    path.map(|path| path.display().to_string())
}

/// What a derived command read.
fn projection(typed: Typed) -> Projection {
    match typed {
        Typed::Init(init) => (
            Vec::new(),
            switches(&[("--write", init.write), ("--agents", init.agents)]),
            Vec::new(),
        ),
        Typed::Doctor(doctor) => (
            Vec::new(),
            Vec::new(),
            values(&[("--profile", doctor.profile.profile)]),
        ),
        Typed::Projects => (Vec::new(), Vec::new(), Vec::new()),
        Typed::Mcp(mcp) => (Vec::new(), Vec::new(), values(&[("--tools", mcp.tools)])),
        Typed::Update(update) => (
            Vec::new(),
            switches(&[("--apply", update.apply.apply)]),
            Vec::new(),
        ),
        Typed::Guide(guide) => (
            guide.operands,
            switches(&[("--json", guide.json.json)]),
            values(&[
                ("--section", guide.section),
                ("--search", guide.search),
                ("--limit", guide.limit),
            ]),
        ),
        Typed::Help(help) => (
            help.operands,
            switches(&[("--refresh", help.refresh), ("--json", help.json.json)]),
            values(&[
                ("--version", help.version),
                ("--limit", help.limit),
                ("--section", help.section),
                ("--profile", help.profile.profile),
            ]),
        ),
        Typed::Javadoc(javadoc) => (
            javadoc.operands,
            switches(&[
                ("--refresh", javadoc.refresh),
                ("--json", javadoc.json.json),
            ]),
            values(&[("--member", javadoc.member), ("--limit", javadoc.limit)]),
        ),
        Typed::Package(package) => (
            package.operands,
            switches(&[
                ("--backend-only", package.backend_only),
                ("--frontend-only", package.frontend_only),
                ("--editable", package.editable),
                ("--force", package.force),
            ]),
            values(&[
                ("--project", package.project.project),
                ("--out", shown(package.out.as_ref())),
            ]),
        ),
    }
}

/// While commands move from GRAMMAR rows to structs: each moved command line, read by the
/// builder's [`spec::args_from`] with the row the command had and by its struct, says the same
/// (operands, switches, values). Remove a group's rows here when the builder is deleted.
#[test]
fn derived_commands_read_what_their_grammar_rows_read() {
    const ROWS: &[(&str, bool, &[&str])] = &[
        ("init", false, &["--write", "--agents"]),
        ("doctor", false, &["--profile"]),
        ("projects", false, &[]),
        ("mcp", false, &["--tools"]),
        ("update", false, &["--apply"]),
        (
            "guide",
            true,
            &["--section", "--search", "--limit", "--json"],
        ),
        (
            "help",
            true,
            &[
                "--version",
                "--limit",
                "--section",
                "--refresh",
                "--json",
                "--profile",
            ],
        ),
        (
            "javadoc",
            true,
            &["--member", "--limit", "--refresh", "--json"],
        ),
        (
            "package",
            true,
            &[
                "--project",
                "--backend-only",
                "--frontend-only",
                "--editable",
                "--out",
                "--force",
            ],
        ),
    ];
    let lines: &[&[&str]] = &[
        &["init"],
        &["init", "--write"],
        &["init", "--agents"],
        &["init", "--write", "--agents", "--write"],
        &["doctor"],
        &["doctor", "--profile", "prod"],
        &["doctor", "--profile=-x"],
        &["projects"],
        &["mcp"],
        &["mcp", "--tools", "core,entity"],
        &["mcp", "--tools", ""],
        &["update"],
        &["update", "--apply"],
        &["guide"],
        &["guide", "workflow", "--section", "Deploy", "--json"],
        &["guide", "--search", "-x y", "--limit", "3"],
        &["guide", "a", "b", "-1"],
        &["help", "search", "mashup", "binding", "--limit", "5"],
        &[
            "help",
            "page",
            "ThingWorx/Welcome.html",
            "--version",
            "9.6",
            "--section",
            "Intro",
            "--refresh",
            "--json",
            "--profile",
            "prod",
        ],
        &["help"],
        &["javadoc", "search", "Thing", "--limit", "2", "--json"],
        &[
            "javadoc",
            "class",
            "Thing",
            "--member",
            "getName",
            "--refresh",
        ],
        &["package", "bundle", "--out", "dist/a.zip", "--backend-only"],
        &[
            "package",
            "extension",
            "--out=x.zip",
            "--editable",
            "--force",
            "--project",
            "Acme",
            "--frontend-only",
        ],
        &["package", "source-control", "--out", ""],
        &["package", "--", "-weird"],
    ];
    for line in lines {
        let mut all = vec!["twaco"];
        all.extend(*line);
        let matches = spec::tree()
            .try_get_matches_from(&all)
            .unwrap_or_else(|e| panic!("{line:?}: {e}"));
        let (path, _) = spec::matched(&matches);
        // The builder's command, from the row it had (its argument ids are the flag names, the
        // derived ones are the field names), under the same policy.
        let (_, operands, flags) = ROWS.iter().find(|(row, _, _)| *row == path).unwrap();
        let built = spec::decorate(
            clap::Command::new("twaco")
                .subcommand(clap::Command::new(path).args(spec::arguments(flags, *operands))),
        );
        let old_matches = built
            .try_get_matches_from(&all)
            .unwrap_or_else(|e| panic!("{line:?}: {e}"));
        let (_, matched) = spec::matched(&old_matches);
        let old = spec::args_from(flags, *operands, matched).unwrap();
        let mut old_values: Vec<(String, String)> = old
            .values
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        let mut add = |name: &str, value: Option<String>| {
            if let Some(value) = value {
                old_values.push((name.to_string(), value));
            }
        };
        add("--profile", old.profile.clone());
        add("--project", old.project.clone());
        add("--out", shown(old.out.as_ref()));
        add("--timeout", old.timeout.map(|t| t.as_secs().to_string()));
        for param in &old.params {
            add("--param", Some(param.clone()));
        }
        let (names, mut on, mut given) = projection(Typed::from_arg_matches(&matches).unwrap());
        let mut old_flags = old.flags.clone();
        old_flags.sort();
        old_values.sort();
        on.sort();
        given.sort();
        assert_eq!(
            (names, on, given),
            (old.names.clone(), old_flags, old_values),
            "{line:?}"
        );
    }
}
