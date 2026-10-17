use super::super::*;
use super::command::Typed;
use super::entity::entity_delete_force_deprecation;
use super::entity_edit::{Media, Menu, Scheduler, State, Style, Thing, Timer};
use super::entity_new::{New, NewEntity};
use super::mashup::{Mashup, Place};
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
    let Typed::Logs(logs) =
        parse_typed(&["logs", "ScriptLog", "--since", "10m", "--json"]).unwrap()
    else {
        panic!("logs parses as logs");
    };
    assert_eq!(logs.operands, ["ScriptLog"]);
    assert_eq!(logs.since.as_deref(), Some("10m"));
    assert!(logs.json.json);
    let query = parse(&["db", "query", "-q", "select 1", "--timeout", "30"]).unwrap();
    assert_eq!(query.values.get("-q").map(String::as_str), Some("select 1"));
    assert_eq!(query.timeout, Some(Duration::from_secs(30)));
    assert_eq!(
        parse_typed(&["call", "T", "S", "--timeout", "0"]).map(|_| ()),
        Err("--timeout needs a positive whole number of seconds".to_string())
    );
    let Typed::Call(call) = parse_typed(&["call", "T", "S", "--timeout", "30"]).unwrap() else {
        panic!("call parses as call");
    };
    assert_eq!(call.timeout.duration(), Ok(Some(Duration::from_secs(30))));
    assert!(parse(&["deploy", "--only-projects", " , "]).is_err());
    assert_eq!(
        parse_typed(&["sync", "--all", "Acme.T"]).map(|_| ()),
        Err("--all and [\"Acme.T\"] say different things; pass one or the other".to_string())
    );
    // A value may begin with a dash, as a log filter can.
    let Typed::Logs(grep) = parse_typed(&["logs", "ScriptLog", "--grep", "-x"]).unwrap() else {
        panic!("logs parses as logs");
    };
    assert_eq!(grep.grep.as_deref(), Some("-x"));
    // A negative number is an operand; a mistyped flag after operands is still refused.
    let Typed::Call(call) = parse_typed(&["call", "T", "S", "-5"]).unwrap() else {
        panic!("call parses as call");
    };
    assert_eq!(call.operands, ["T", "S", "-5"]);
    assert!(parse_typed(&["sync", "Acme.T", "--cehck"]).is_err());
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
    let Typed::New(New::Entity(NewEntity::Menu(menu))) = parse_typed(&[
        "new",
        "menu",
        "Acme.Main_MN",
        "--item",
        "Home=Acme.Home_MU",
        "--default",
        "--label",
        "Main",
    ])
    .unwrap() else {
        panic!("new menu parses as new menu");
    };
    assert_eq!(menu.base.operands, ["Acme.Main_MN"]);
    assert_eq!(menu.default.as_deref(), Some(""));
    assert_eq!(menu.label.as_deref(), Some("Main"));
    // It may still be negative.
    let Typed::Mashup(Mashup::Param(param)) = parse_typed(&[
        "mashup",
        "param",
        "set",
        "Acme.Home_MU",
        "Count",
        "--default",
        "-1",
    ])
    .unwrap() else {
        panic!("mashup param parses as mashup param");
    };
    assert_eq!(param.default.as_deref(), Some("-1"));
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

/// Each value of a repeated flag, once per time it was given.
fn repeated(name: &str, given: Vec<String>) -> Vec<(String, String)> {
    given
        .into_iter()
        .map(|value| (name.to_string(), value))
        .collect()
}

/// A value that may be given bare (`--default`), as the builder read it: given, it is a switch,
/// and its value is a value unless empty.
fn bare(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.is_empty())
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

        Typed::Catalog(catalog) => (
            catalog.operands,
            switches(&[("--json", catalog.json.json)]),
            values(&[
                ("--project", catalog.project.project),
                ("--search", catalog.search),
            ]),
        ),
        Typed::Impact(impact) => (
            impact.operands,
            switches(&[
                ("--detail", impact.detail.detail),
                ("--json", impact.json.json),
                ("--dot", impact.dot),
            ]),
            values(&[
                ("--member", impact.member),
                ("--min-confidence", impact.min_confidence),
                ("--depth", impact.depth),
            ]),
        ),
        Typed::Trace(trace) => (
            trace.operands,
            switches(&[("--json", trace.json.json)]),
            trace
                .params
                .into_iter()
                .map(|param| ("--param".to_string(), param))
                .collect(),
        ),
        Typed::Unused(unused) => (
            Vec::new(),
            switches(&[
                ("--fields", unused.fields),
                ("--detail", unused.detail.detail),
                ("--json", unused.json.json),
            ]),
            values(&[
                ("--min-confidence", unused.min_confidence),
                ("--collection", unused.collection),
            ]),
        ),
        Typed::Docs(docs) => (
            Vec::new(),
            switches(&[
                ("--detail", docs.detail.detail),
                ("--json", docs.json.json),
                ("--force", docs.force),
                ("--check", docs.check),
            ]),
            values(&[("--out", shown(docs.out.as_ref()))]),
        ),
        Typed::Widgets(widgets) => (
            widgets.operands,
            switches(&[("--json", widgets.json.json)]),
            values(&[("--search", widgets.search)]),
        ),

        Typed::Extract(extract) => (
            extract.selection.operands,
            switches(&[("--all", extract.selection.all)]),
            values(&[("--project", extract.project.project)]),
        ),
        Typed::Sync(sync) => (
            sync.selection.operands,
            switches(&[
                ("--all", sync.selection.all),
                ("--check", sync.check),
                ("--allow-add-remove", sync.allow_add_remove),
                ("--relayout", sync.relayout),
            ]),
            values(&[("--project", sync.project.project)]),
        ),
        Typed::Fmt(fmt) => (Vec::new(), switches(&[("--check", fmt.check)]), Vec::new()),
        Typed::Check(check) => (
            Vec::new(),
            switches(&[("--detail", check.detail.detail), ("--live", check.live)]),
            values(&[("--profile", check.profile.profile)]),
        ),
        Typed::Types(types) => (
            Vec::new(),
            switches(&[
                ("--platform", types.platform),
                ("--check", types.check),
                ("--json", types.json.json),
            ]),
            values(&[("--profile", types.profile.profile)]),
        ),

        Typed::Users(users) => (
            users.operands,
            switches(&[("--apply", users.apply.apply), ("--json", users.json.json)]),
            values(&[("--profile", users.profile.profile)]),
        ),
        Typed::Test(test) => (
            test.operands,
            switches(&[
                ("--apply", test.apply.apply),
                ("--json", test.json.json),
                ("--detail", test.detail.detail),
            ]),
            values(&[("--profile", test.profile.profile)]),
        ),
        Typed::Localization(localization) => (
            localization.operands,
            switches(&[
                ("--prune", localization.prune),
                ("--json", localization.json.json),
                ("--detail", localization.detail.detail),
                ("--apply", localization.apply.apply),
            ]),
            values(&[
                ("--table", localization.table),
                ("--project", localization.project.project),
                ("--language-common", localization.language_common),
                ("--language-native", localization.language_native),
                ("--description", localization.description),
                ("--value", localization.value),
                ("--usage", localization.usage),
                ("--context", localization.context),
                ("--profile", localization.profile.profile),
            ]),
        ),

        Typed::Repo(repo) => (
            repo.operands,
            switches(&[
                ("--recursive", repo.recursive),
                ("--json", repo.json.json),
                ("--force", repo.force),
                ("--overwrite", repo.overwrite),
                ("--apply", repo.apply.apply),
            ]),
            values(&[
                ("--out", shown(repo.out.as_ref())),
                ("--profile", repo.profile.profile),
            ]),
        ),
        Typed::Ext(ext) => (
            ext.operands,
            switches(&[("--apply", ext.apply.apply), ("--json", ext.json.json)]),
            values(&[("--profile", ext.profile.profile)]),
        ),
        Typed::Search(search) => (
            search.operands,
            switches(&[("--json", search.json.json)]),
            values(&[
                ("--type", search.kind),
                ("--project", search.project.project),
                ("--limit", search.limit),
                ("--profile", search.profile.profile),
            ]),
        ),
        Typed::Export(export) => (
            export.operands,
            switches(&[
                ("--force", export.force),
                ("--with-dependents", export.with_dependents),
                ("--apply", export.apply.apply),
            ]),
            values(&[
                ("--out", shown(export.out.as_ref())),
                ("--project", export.project.project),
                ("--profile", export.profile.profile),
                ("--repository", export.repository),
                ("--path", export.path),
                ("--collection", export.collection),
                ("--tags", export.tags),
                ("--zip", export.zip),
            ]),
        ),
        Typed::Import(import) => (
            import.operands,
            switches(&[
                ("--apply", import.apply.apply),
                ("--overwrite-properties", import.overwrite_properties),
                ("--overwrite-tables", import.overwrite_tables),
                ("--detail", import.detail.detail),
            ]),
            values(&[
                ("--repository", import.repository),
                ("--path", import.path),
                ("--profile", import.profile.profile),
            ]),
        ),

        Typed::Call(call) => (
            call.operands,
            switches(&[
                ("--detail", call.detail.detail),
                ("--with-logs", call.with_logs),
            ]),
            values(&[
                // args_from read it into a Duration; its seconds are what was given.
                (
                    "--timeout",
                    call.timeout
                        .duration()
                        .unwrap()
                        .map(|timeout| timeout.as_secs().to_string()),
                ),
                ("--profile", call.profile.profile),
                ("--as", call.user),
            ]),
        ),
        Typed::Logs(logs) => (
            logs.operands,
            switches(&[
                ("--oldest-first", logs.oldest_first),
                ("--json", logs.json.json),
                ("--reset", logs.reset),
                ("--apply", logs.apply.apply),
            ]),
            values(&[
                ("--since", logs.since),
                ("--from", logs.from),
                ("--to", logs.to),
                ("--level", logs.level),
                ("--grep", logs.grep),
                ("--regex", logs.regex),
                ("--user", logs.user),
                ("--thread", logs.thread),
                ("--origin", logs.origin),
                ("--limit", logs.limit),
                ("--profile", logs.profile.profile),
                ("--sublogger", logs.sublogger),
            ]),
        ),
        Typed::Settings(settings) => (
            settings.operands,
            switches(&[("--json", settings.json.json)]),
            values(&[
                ("--search", settings.search),
                ("--profile", settings.profile.profile),
            ]),
        ),
        Typed::Thing(thing) => thing_projection(thing),
        Typed::New(new) => new_projection(new),
        Typed::Mashup(mashup) => mashup_projection(mashup),
        Typed::Timer(Timer::Set(set)) => (
            set.operands,
            switches(&[
                ("--enabled", set.enabled),
                ("--disabled", set.disabled),
                ("--apply", set.apply.apply),
                ("--json", set.json.json),
            ]),
            values(&[("--every", set.every), ("--run-as", set.run_as)]),
        ),
        Typed::Scheduler(Scheduler::Set(set)) => (
            set.operands,
            switches(&[
                ("--enabled", set.enabled),
                ("--disabled", set.disabled),
                ("--apply", set.apply.apply),
                ("--json", set.json.json),
            ]),
            values(&[("--cron", set.cron), ("--run-as", set.run_as)]),
        ),
        Typed::Style(Style::Set(set)) => (
            set.operands,
            switches(&[
                ("--bold", set.bold),
                ("--no-bold", set.no_bold),
                ("--italic", set.italic),
                ("--no-italic", set.no_italic),
                ("--underline", set.underline),
                ("--no-underline", set.no_underline),
                ("--apply", set.apply.apply),
                ("--json", set.json.json),
            ]),
            values(&[
                ("--background", set.background),
                ("--foreground", set.foreground),
                ("--secondary-background", set.secondary_background),
                ("--line-color", set.line_color),
                ("--line-style", set.line_style),
                ("--line-thickness", set.line_thickness),
                ("--text-size", set.text_size),
                ("--image", set.image),
                ("--display-string", set.display_string),
            ]),
        ),
        Typed::Media(Media::Set(set)) => (
            set.operands,
            switches(&[("--apply", set.apply.apply), ("--json", set.json.json)]),
            values(&[("--file", shown(set.file.as_ref()))]),
        ),
        Typed::Menu(Menu::Item(item)) => (
            item.operands,
            switches(&[
                ("--default", item.default.is_some()),
                ("--apply", item.apply.apply),
                ("--json", item.json.json),
            ]),
            values(&[("--default", bare(item.default)), ("--before", item.before)]),
        ),
        Typed::State(State::Set(set)) => {
            let mut given = values(&[("--type", set.kind)]);
            given.extend(repeated("--state", set.states));
            (
                set.operands,
                switches(&[("--apply", set.apply.apply), ("--json", set.json.json)]),
                given,
            )
        }
    }
}

/// What a `thing` command read.
fn thing_projection(thing: Thing) -> Projection {
    match thing {
        Thing::Property(property) => (
            property.operands,
            switches(&[
                ("--default", property.default.is_some()),
                ("--no-default", property.no_default),
                ("--persistent", property.persistent),
                ("--no-persistent", property.no_persistent),
                ("--logged", property.logged),
                ("--no-logged", property.no_logged),
                ("--read-only", property.read_only),
                ("--no-read-only", property.no_read_only),
                ("--force", property.force),
                ("--apply", property.apply.apply),
                ("--json", property.json.json),
            ]),
            values(&[
                ("--default", bare(property.default)),
                ("--category", property.category),
                ("--units", property.units),
                ("--data-shape", property.data_shape),
                ("--description", property.description),
            ]),
        ),
        Thing::Subscription(subscription) => (
            subscription.operands,
            switches(&[
                ("--apply", subscription.apply.apply),
                ("--json", subscription.json.json),
            ]),
            values(&[
                ("--event", subscription.event),
                ("--source", subscription.source),
                ("--source-property", subscription.source_property),
                ("--then", subscription.then),
                ("--script", shown(subscription.script.as_ref())),
            ]),
        ),
        Thing::Shape(shape) => (
            shape.operands,
            switches(&[
                ("--force", shape.force),
                ("--apply", shape.apply.apply),
                ("--json", shape.json.json),
            ]),
            Vec::new(),
        ),
    }
}

/// What a `new` command read.
fn new_projection(new: New) -> Projection {
    let new = match new {
        New::BuildingBlock(block) => {
            return (
                block.operands,
                switches(&[
                    ("--model-logic", block.model_logic),
                    ("--no-management-shape", block.no_management_shape),
                    ("--apply", block.apply.apply),
                    ("--json", block.json.json),
                ]),
                values(&[
                    ("--type", block.kind),
                    ("--display-name", block.display_name),
                    ("--description", block.description),
                    ("--parent", block.parent),
                    ("--root", block.root),
                    ("--base-extension", block.base_extension),
                ]),
            )
        }
        New::Entity(new) => new,
    };
    let (base, mut on, mut given) = match new {
        NewEntity::Thing(new) => {
            let mut given = values(&[
                ("--template", new.template),
                ("--value-stream", new.value_stream),
            ]);
            given.extend(repeated("--visibility", new.visibility));
            given.extend(repeated("--shape", new.shapes));
            (new.base, Vec::new(), given)
        }
        NewEntity::ThingTemplate(new) => {
            let mut given = values(&[("--base", new.parent)]);
            given.extend(repeated("--visibility", new.visibility));
            given.extend(repeated("--shape", new.shapes));
            (new.base, Vec::new(), given)
        }
        NewEntity::ThingShape(new) => (
            new.base,
            Vec::new(),
            repeated("--visibility", new.visibility),
        ),
        NewEntity::DataShape(new) => {
            let mut given = values(&[("--base", new.parent)]);
            given.extend(repeated("--visibility", new.visibility));
            given.extend(repeated("--field", new.fields));
            (new.base, Vec::new(), given)
        }
        NewEntity::Timer(new) => {
            let mut given = values(&[
                ("--every", new.every),
                ("--run-as", new.event.run_as),
                ("--then", new.event.then),
                ("--script", shown(new.event.script.as_ref())),
            ]);
            given.extend(repeated("--visibility", new.visibility));
            (
                new.base,
                switches(&[("--disabled", new.event.disabled)]),
                given,
            )
        }
        NewEntity::Scheduler(new) => {
            let mut given = values(&[
                ("--cron", new.cron),
                ("--run-as", new.event.run_as),
                ("--then", new.event.then),
                ("--script", shown(new.event.script.as_ref())),
            ]);
            given.extend(repeated("--visibility", new.visibility));
            (
                new.base,
                switches(&[("--disabled", new.event.disabled)]),
                given,
            )
        }
        NewEntity::DataTable(new) => {
            let mut given = values(&[("--data-shape", new.data_shape)]);
            given.extend(repeated("--index", new.indexes));
            (new.base, Vec::new(), given)
        }
        NewEntity::Stream(new) => (
            new.base,
            Vec::new(),
            values(&[("--data-shape", new.data_shape)]),
        ),
        NewEntity::ValueStream(new) => (new.base, Vec::new(), Vec::new()),
        NewEntity::Media(new) => (
            new.base,
            Vec::new(),
            values(&[("--file", shown(new.file.as_ref()))]),
        ),
        NewEntity::Style(new) => (
            new.base,
            switches(&[
                ("--bold", new.bold),
                ("--italic", new.italic),
                ("--underline", new.underline),
            ]),
            values(&[
                ("--background", new.background),
                ("--foreground", new.foreground),
                ("--secondary-background", new.secondary_background),
                ("--line-color", new.line_color),
                ("--line-style", new.line_style),
                ("--line-thickness", new.line_thickness),
                ("--text-size", new.text_size),
                ("--image", new.image),
                ("--display-string", new.display_string),
            ]),
        ),
        NewEntity::State(new) => {
            let mut given = values(&[("--type", new.kind)]);
            given.extend(repeated("--state", new.states));
            (new.base, Vec::new(), given)
        }
        NewEntity::Menu(new) => {
            let mut given = values(&[
                ("--default", bare(new.default.clone())),
                ("--label", new.label),
            ]);
            given.extend(repeated("--item", new.items));
            (
                new.base,
                switches(&[("--default", new.default.is_some())]),
                given,
            )
        }
        NewEntity::Theme(new) => {
            let mut given = values(&[("--from", new.from)]);
            given.extend(repeated("--prop", new.props));
            (new.base, Vec::new(), given)
        }
    };
    on.extend(switches(&[
        ("--apply", base.apply.apply),
        ("--json", base.json.json),
    ]));
    given.extend(values(&[
        ("--project", base.project.project),
        ("--description", base.description),
    ]));
    (base.operands, on, given)
}

/// Where a mashup command places what it adds.
fn placed(place: Place) -> Vec<(String, String)> {
    values(&[
        ("--in", place.inside),
        ("--right-of", place.right_of),
        ("--left-of", place.left_of),
        ("--above", place.above),
        ("--below", place.below),
    ])
}

/// What a `mashup` command read.
fn mashup_projection(mashup: Mashup) -> Projection {
    match mashup {
        Mashup::Validate(validate) => (
            validate.selection.operands,
            switches(&[
                ("--all", validate.selection.all),
                ("--json", validate.json.json),
            ]),
            Vec::new(),
        ),
        Mashup::Describe(describe) => (
            describe.operands,
            switches(&[("--json", describe.json.json)]),
            Vec::new(),
        ),
        Mashup::Add(add) => {
            let mut given = placed(add.place);
            given.extend(values(&[("--label", add.label)]));
            given.extend(repeated("--prop", add.props));
            given.extend(repeated("--input", add.inputs));
            (
                add.operands,
                switches(&[("--apply", add.apply.apply), ("--json", add.json.json)]),
                given,
            )
        }
        Mashup::AddContained(add) => {
            let mut given = placed(add.place);
            given.extend(repeated("--set", add.props));
            (
                add.operands,
                switches(&[("--apply", add.apply.apply), ("--json", add.json.json)]),
                given,
            )
        }
        Mashup::Bind(bind) => (
            bind.operands,
            switches(&[("--apply", bind.apply.apply), ("--json", bind.json.json)]),
            values(&[("--from", bind.from), ("--to", bind.to)]),
        ),
        Mashup::On(on) => (
            on.operands,
            switches(&[("--apply", on.apply.apply), ("--json", on.json.json)]),
            values(&[("--event", on.event), ("--invoke", on.invoke)]),
        ),
        Mashup::Param(param) => (
            param.operands,
            switches(&[
                ("--default", param.default.is_some()),
                ("--no-default", param.no_default),
                ("--force", param.force),
                ("--apply", param.apply.apply),
                ("--json", param.json.json),
            ]),
            values(&[
                ("--type", param.kind),
                ("--direction", param.direction),
                ("--description", param.description),
                ("--default", bare(param.default)),
                ("--data-shape", param.data_shape),
            ]),
        ),
        Mashup::Recipe(recipe) => {
            let mut given = values(&[
                ("--if", recipe.condition),
                ("--expr", recipe.expression),
                ("--after", recipe.after),
                ("--say", recipe.say),
                ("--from", recipe.from),
                ("--kind", recipe.kind),
                ("--then", recipe.then),
                ("--title", recipe.title),
                ("--on", recipe.on),
                ("--mashup", recipe.mashup),
                ("--window", recipe.window),
                ("--size", recipe.size),
                ("--reload", recipe.reload),
                ("--every", recipe.every),
                ("--interval", recipe.interval),
                ("--else-say", recipe.else_say),
                ("--data", recipe.data),
                ("--x", recipe.x),
                ("--value", recipe.value),
                ("--selected", recipe.selected),
            ]);
            given.extend(repeated("--input", recipe.inputs));
            given.extend(repeated("--pass", recipe.pass));
            given.extend(repeated("--column", recipe.columns));
            (
                recipe.operands,
                switches(&[
                    ("--apply", recipe.apply.apply),
                    ("--json", recipe.json.json),
                ]),
                given,
            )
        }
        Mashup::Event(event) => (
            event.operands,
            switches(&[("--apply", event.apply.apply), ("--json", event.json.json)]),
            values(&[("--when", event.when)]),
        ),
        Mashup::Templates(templates) => (
            templates.operands,
            switches(&[("--json", templates.json.json)]),
            Vec::new(),
        ),
        Mashup::New(new) => {
            let mut given = values(&[
                ("--project", new.project.project),
                ("--template", new.template),
                ("--layout", new.layout),
                ("--primary-menu", new.primary_menu),
                ("--secondary-menu", new.secondary_menu),
                ("--theme", new.theme),
            ]);
            given.extend(repeated("--visibility", new.visibility));
            given.extend(repeated("--param", new.params));
            given.extend(repeated("--tab", new.tabs));
            (
                new.operands,
                switches(&[
                    ("--master", new.master),
                    ("--apply", new.apply.apply),
                    ("--json", new.json.json),
                ]),
                given,
            )
        }
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
        ("catalog", true, &["--project", "--search", "--json"]),
        (
            "impact",
            true,
            &[
                "--member",
                "--min-confidence",
                "--depth",
                "--detail",
                "--json",
                "--dot",
            ],
        ),
        ("trace", true, &["--param", "--json"]),
        (
            "unused",
            false,
            &[
                "--min-confidence",
                "--collection",
                "--fields",
                "--detail",
                "--json",
            ],
        ),
        (
            "docs",
            false,
            &["--detail", "--json", "--out", "--force", "--check"],
        ),
        ("widgets", true, &["--search", "--json"]),
        ("extract", true, &["--all", "--project"]),
        (
            "sync",
            true,
            &[
                "--all",
                "--check",
                "--allow-add-remove",
                "--relayout",
                "--project",
            ],
        ),
        ("fmt", false, &["--check"]),
        ("check", false, &["--detail", "--live", "--profile"]),
        (
            "types",
            false,
            &["--platform", "--check", "--json", "--profile"],
        ),
        ("users", true, &["--apply", "--json", "--profile"]),
        (
            "test",
            true,
            &["--apply", "--json", "--detail", "--profile"],
        ),
        (
            "localization",
            true,
            &[
                "--table",
                "--project",
                "--prune",
                "--language-common",
                "--language-native",
                "--description",
                "--value",
                "--usage",
                "--context",
                "--json",
                "--detail",
                "--profile",
                "--apply",
            ],
        ),
        (
            "repo",
            true,
            &[
                "--recursive",
                "--out",
                "--json",
                "--profile",
                "--force",
                "--overwrite",
                "--apply",
            ],
        ),
        ("ext", true, &["--apply", "--json", "--profile"]),
        (
            "search",
            true,
            &["--type", "--project", "--limit", "--json", "--profile"],
        ),
        (
            "export",
            true,
            &[
                "--out",
                "--force",
                "--project",
                "--profile",
                "--repository",
                "--path",
                "--collection",
                "--tags",
                "--zip",
                "--with-dependents",
                "--apply",
            ],
        ),
        (
            "import",
            true,
            &[
                "--apply",
                "--overwrite-properties",
                "--overwrite-tables",
                "--repository",
                "--path",
                "--profile",
                "--detail",
            ],
        ),
        (
            "call",
            true,
            &["--timeout", "--detail", "--profile", "--with-logs", "--as"],
        ),
        (
            "logs",
            true,
            &[
                "--since",
                "--from",
                "--to",
                "--level",
                "--grep",
                "--regex",
                "--user",
                "--thread",
                "--origin",
                "--limit",
                "--oldest-first",
                "--json",
                "--profile",
                "--sublogger",
                "--reset",
                "--apply",
            ],
        ),
        ("settings", true, &["--search", "--json", "--profile"]),
        (
            "thing property",
            true,
            &[
                "--default",
                "--no-default",
                "--persistent",
                "--no-persistent",
                "--logged",
                "--no-logged",
                "--read-only",
                "--no-read-only",
                "--category",
                "--units",
                "--data-shape",
                "--description",
                "--force",
                "--apply",
                "--json",
            ],
        ),
        (
            "thing subscription",
            true,
            &[
                "--event",
                "--source",
                "--source-property",
                "--then",
                "--script",
                "--apply",
                "--json",
            ],
        ),
        ("thing shape", true, &["--force", "--apply", "--json"]),
        (
            "timer set",
            true,
            &[
                "--every",
                "--run-as",
                "--enabled",
                "--disabled",
                "--apply",
                "--json",
            ],
        ),
        (
            "scheduler set",
            true,
            &[
                "--cron",
                "--run-as",
                "--enabled",
                "--disabled",
                "--apply",
                "--json",
            ],
        ),
        (
            "style set",
            true,
            &[
                "--background",
                "--foreground",
                "--secondary-background",
                "--line-color",
                "--line-style",
                "--line-thickness",
                "--bold",
                "--no-bold",
                "--italic",
                "--no-italic",
                "--underline",
                "--no-underline",
                "--text-size",
                "--image",
                "--display-string",
                "--apply",
                "--json",
            ],
        ),
        ("media set", true, &["--file", "--apply", "--json"]),
        (
            "menu item",
            true,
            &["--default", "--before", "--apply", "--json"],
        ),
        (
            "state set",
            true,
            &["--type", "--state", "--apply", "--json"],
        ),
        (
            "new building-block",
            true,
            &[
                "--type",
                "--display-name",
                "--description",
                "--parent",
                "--root",
                "--base-extension",
                "--model-logic",
                "--no-management-shape",
                "--apply",
                "--json",
            ],
        ),
        (
            "new thing",
            true,
            &[
                "--project",
                "--description",
                "--visibility",
                "--template",
                "--shape",
                "--value-stream",
                "--apply",
                "--json",
            ],
        ),
        (
            "new thing-template",
            true,
            &[
                "--project",
                "--description",
                "--visibility",
                "--base",
                "--shape",
                "--apply",
                "--json",
            ],
        ),
        (
            "new thing-shape",
            true,
            &[
                "--project",
                "--description",
                "--visibility",
                "--apply",
                "--json",
            ],
        ),
        (
            "new data-shape",
            true,
            &[
                "--project",
                "--description",
                "--visibility",
                "--base",
                "--field",
                "--apply",
                "--json",
            ],
        ),
        (
            "new timer",
            true,
            &[
                "--project",
                "--description",
                "--visibility",
                "--every",
                "--run-as",
                "--disabled",
                "--then",
                "--script",
                "--apply",
                "--json",
            ],
        ),
        (
            "new scheduler",
            true,
            &[
                "--project",
                "--description",
                "--visibility",
                "--cron",
                "--run-as",
                "--disabled",
                "--then",
                "--script",
                "--apply",
                "--json",
            ],
        ),
        (
            "new data-table",
            true,
            &[
                "--project",
                "--description",
                "--data-shape",
                "--index",
                "--apply",
                "--json",
            ],
        ),
        (
            "new stream",
            true,
            &[
                "--project",
                "--description",
                "--data-shape",
                "--apply",
                "--json",
            ],
        ),
        (
            "new value-stream",
            true,
            &["--project", "--description", "--apply", "--json"],
        ),
        (
            "new media",
            true,
            &["--project", "--description", "--file", "--apply", "--json"],
        ),
        (
            "new style",
            true,
            &[
                "--project",
                "--description",
                "--background",
                "--foreground",
                "--secondary-background",
                "--line-color",
                "--line-style",
                "--line-thickness",
                "--bold",
                "--italic",
                "--underline",
                "--text-size",
                "--image",
                "--display-string",
                "--apply",
                "--json",
            ],
        ),
        (
            "new state",
            true,
            &[
                "--project",
                "--description",
                "--type",
                "--state",
                "--apply",
                "--json",
            ],
        ),
        (
            "new menu",
            true,
            &[
                "--project",
                "--description",
                "--item",
                "--default",
                "--label",
                "--apply",
                "--json",
            ],
        ),
        (
            "new theme",
            true,
            &[
                "--project",
                "--description",
                "--from",
                "--prop",
                "--apply",
                "--json",
            ],
        ),
        ("mashup validate", true, &["--all", "--json"]),
        ("mashup describe", true, &["--json"]),
        (
            "mashup add",
            true,
            &[
                "--in",
                "--right-of",
                "--left-of",
                "--above",
                "--below",
                "--label",
                "--prop",
                "--input",
                "--apply",
                "--json",
            ],
        ),
        (
            "mashup add-contained",
            true,
            &[
                "--in",
                "--right-of",
                "--left-of",
                "--above",
                "--below",
                "--set",
                "--apply",
                "--json",
            ],
        ),
        (
            "mashup bind",
            true,
            &["--from", "--to", "--apply", "--json"],
        ),
        (
            "mashup on",
            true,
            &["--event", "--invoke", "--apply", "--json"],
        ),
        (
            "mashup param",
            true,
            &[
                "--type",
                "--direction",
                "--description",
                "--default",
                "--no-default",
                "--data-shape",
                "--force",
                "--apply",
                "--json",
            ],
        ),
        (
            "mashup recipe",
            true,
            &[
                "--if",
                "--expr",
                "--input",
                "--after",
                "--say",
                "--from",
                "--kind",
                "--then",
                "--title",
                "--on",
                "--mashup",
                "--window",
                "--size",
                "--pass",
                "--reload",
                "--every",
                "--interval",
                "--else-say",
                "--data",
                "--x",
                "--value",
                "--column",
                "--selected",
                "--apply",
                "--json",
            ],
        ),
        ("mashup event", true, &["--when", "--apply", "--json"]),
        ("mashup templates", true, &["--json"]),
        (
            "mashup new",
            true,
            &[
                "--project",
                "--template",
                "--layout",
                "--primary-menu",
                "--secondary-menu",
                "--theme",
                "--master",
                "--visibility",
                "--param",
                "--tab",
                "--apply",
                "--json",
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
        &["catalog"],
        &[
            "catalog",
            "Acme.T",
            "--project",
            "Acme",
            "--search",
            "-get",
            "--json",
        ],
        &[
            "impact",
            "Acme.T",
            "--member",
            "Run",
            "--min-confidence",
            "resolved",
            "--depth",
            "-1",
            "--detail",
            "--json",
            "--dot",
        ],
        &["impact", "a", "b"],
        &["trace", "Acme.T.Run"],
        &[
            "trace",
            "Acme.T.Run",
            "--param",
            "a:STRING",
            "--param",
            "b",
            "--json",
        ],
        &["unused"],
        &[
            "unused",
            "--min-confidence",
            "review",
            "--collection",
            "Things",
            "--detail",
            "--json",
            "--json",
        ],
        &["unused", "--fields"],
        &["docs"],
        &[
            "docs",
            "--out",
            "SOLUTION.md",
            "--check",
            "--detail",
            "--json",
        ],
        &["docs", "--out=", "--force"],
        &["widgets"],
        &["widgets", "ptcsbutton", "--json"],
        &["widgets", "--search", "grid"],
        &["extract"],
        &["extract", "--all", "--project", "Acme"],
        &["extract", "Acme.A", "Acme.B"],
        &["extract", "--all", "Acme.A"],
        &[
            "sync",
            "--all",
            "--check",
            "--allow-add-remove",
            "--relayout",
            "--check",
        ],
        &["sync", "Acme.A", "--project=Acme"],
        &["sync", "Acme.A", "--all"],
        &["fmt"],
        &["fmt", "--check"],
        &["check"],
        &["check", "--detail", "--live", "--profile", "qa"],
        &["types"],
        &["types", "--platform", "--profile", "qa"],
        &["types", "--check", "--json"],
        &["users", "list", "--json"],
        &["users", "seed", "--apply", "--profile", "qa"],
        &["users"],
        &["test"],
        &[
            "test",
            "Orders",
            "Users",
            "--apply",
            "--json",
            "--detail",
            "--profile=qa",
        ],
        &[
            "localization",
            "status",
            "--table",
            "Acme",
            "--json",
            "--detail",
        ],
        &[
            "localization",
            "pull",
            "--prune",
            "--apply",
            "--profile",
            "qa",
        ],
        &[
            "localization",
            "new",
            "Acme",
            "--project",
            "Acme",
            "--language-common",
            "German",
            "--language-native",
            "Deutsch",
            "--description",
            "-",
        ],
        &[
            "localization",
            "set",
            "Acme",
            "key",
            "--value",
            "-1",
            "--usage",
            "label",
            "--context",
            "",
        ],
        &["repo", "list", "--json"],
        &["repo", "ls", "Acme", "/", "--recursive", "--profile", "qa"],
        &["repo", "get", "Acme", "a.txt", "--out", "b.txt", "--force"],
        &[
            "repo",
            "put",
            "Acme",
            "a.txt",
            "/a.txt",
            "--overwrite",
            "--apply",
        ],
        &["ext", "list", "--json"],
        &["ext", "import", "x.zip", "--apply", "--profile", "qa"],
        &["search"],
        &[
            "search",
            "Orders",
            "Manager",
            "--type",
            "Thing,DataShape",
            "--project",
            "Acme",
            "--limit",
            "5",
            "--json",
        ],
        &[
            "export",
            "entity",
            "Things/Acme.T",
            "--out",
            "t.xml",
            "--force",
            "--profile",
            "qa",
        ],
        &[
            "export",
            "source-control",
            "--repository",
            "SystemRepository",
            "--path",
            "/out",
            "--project",
            "Acme",
            "--collection",
            "Things",
            "--tags",
            "Acme:v1",
            "--zip",
            "out.zip",
            "--with-dependents",
            "--apply",
        ],
        &[
            "import",
            "a.xml",
            "--apply",
            "--overwrite-properties",
            "--overwrite-tables",
            "--detail",
        ],
        &[
            "import",
            "source-control",
            "--repository",
            "R",
            "--path",
            "-p",
            "--profile",
            "qa",
        ],
        &["call", "Acme.T", "Run"],
        &[
            "call",
            "Acme.T",
            "Run",
            "{\"a\": -1}",
            "--timeout",
            "30",
            "--detail",
            "--with-logs",
            "--as",
            "tester",
            "--profile",
            "qa",
        ],
        &["call", "Acme.T", "Run", "-5"],
        &["call", "Acme.T", "Run", "--timeout", "0"],
        &["call", "Acme.T", "Run", "--timeout", "-3"],
        &["call", "Acme.T", "Run", "--timeout", "soon"],
        &[
            "logs",
            "ScriptLog",
            "--since",
            "10m",
            "--level",
            "WARN",
            "--grep",
            "-x",
            "--user",
            "Admin",
            "--json",
        ],
        &[
            "logs",
            "ScriptLog",
            "--from",
            "-2h",
            "--to",
            "now",
            "--regex",
            "^-",
            "--thread",
            "t",
            "--origin",
            "o",
            "--limit",
            "-1",
            "--oldest-first",
        ],
        &[
            "logs",
            "level",
            "ScriptLog",
            "DEBUG",
            "--sublogger",
            "Acme",
            "--apply",
            "--profile",
            "qa",
        ],
        &["logs", "level", "ScriptLog", "--reset"],
        &["settings"],
        &["settings", "Subsystem", "Table", "--json"],
        &["settings", "--search", "-timeout", "--profile", "qa"],
        &[
            "thing",
            "property",
            "add",
            "Acme.T",
            "Count:INTEGER",
            "--default",
            "-1",
            "--persistent",
            "--no-logged",
            "--read-only",
            "--category",
            "c",
            "--units",
            "u",
            "--data-shape",
            "Acme.DS",
            "--description",
            "-d",
            "--apply",
            "--json",
        ],
        &["thing", "property", "set", "Acme.T", "Count", "--default"],
        &[
            "thing",
            "property",
            "set",
            "Acme.T",
            "Count",
            "--default",
            "",
        ],
        &[
            "thing",
            "property",
            "set",
            "Acme.T",
            "Count",
            "--default",
            "--no-default",
            "--no-persistent",
            "--logged",
            "--no-read-only",
            "--force",
        ],
        &["thing", "property", "remove", "Acme.T", "Count", "--force"],
        &[
            "thing",
            "subscription",
            "add",
            "Acme.T",
            "OnChange",
            "--event",
            "DataChange",
            "--source",
            "Acme.S",
            "--source-property",
            "p",
            "--then",
            "me.x = -1;",
        ],
        &[
            "thing",
            "subscription",
            "add",
            "Acme.T",
            "OnChange",
            "--script",
            "on.js",
            "--apply",
            "--json",
        ],
        &[
            "thing", "shape", "add", "Acme.T", "Acme.S", "--force", "--apply",
        ],
        &["thing", "shape"],
        &[
            "timer",
            "set",
            "Acme.Timer",
            "--every",
            "60",
            "--run-as",
            "Admin",
            "--enabled",
            "--apply",
        ],
        &["timer", "set", "Acme.Timer", "--disabled", "--json"],
        &[
            "scheduler",
            "set",
            "Acme.S",
            "--cron",
            "0 0 * * * ?",
            "--run-as",
            "Admin",
            "--disabled",
            "--enabled",
        ],
        &[
            "style",
            "set",
            "Acme.Style",
            "--background",
            "#fff",
            "--foreground",
            "#000",
            "--secondary-background",
            "#eee",
            "--line-color",
            "#111",
            "--line-style",
            "dashed",
            "--line-thickness",
            "2",
            "--bold",
            "--no-italic",
            "--underline",
            "--text-size",
            "large",
            "--image",
            "Acme.M",
            "--display-string",
            "-",
            "--apply",
            "--json",
        ],
        &[
            "style",
            "set",
            "Acme.Style",
            "--no-bold",
            "--italic",
            "--no-underline",
        ],
        &[
            "media", "set", "Acme.M", "--file", "logo.png", "--apply", "--json",
        ],
        &["media", "set", "Acme.M", "--file", ""],
        &[
            "menu",
            "item",
            "add",
            "Acme.Main_MN",
            "Home=Acme.Home_MU",
            "--default",
            "--before",
            "About",
            "--apply",
        ],
        &[
            "menu",
            "item",
            "add",
            "Acme.Main_MN",
            "Home=H",
            "--default",
            "x",
        ],
        &["menu", "item", "remove", "Acme.Main_MN", "Home", "--json"],
        &[
            "state", "set", "Acme.SD", "--type", "numeric", "--state", "a", "--state", "-b",
            "--apply", "--json",
        ],
        &["state", "set", "Acme.SD"],
        &[
            "new",
            "building-block",
            "Acme.Orders",
            "--type",
            "abstract",
            "--display-name",
            "Orders",
            "--description",
            "-",
            "--parent",
            "Acme.Base",
            "--root",
            "blocks/orders",
            "--base-extension",
            "x",
            "--model-logic",
            "--no-management-shape",
            "--apply",
            "--json",
        ],
        &["new", "building-block"],
        &[
            "new",
            "thing",
            "Acme.T",
            "--project",
            "Acme",
            "--description",
            "d",
            "--visibility",
            "Org:Acme",
            "--visibility",
            "-x",
            "--template",
            "GenericThing",
            "--shape",
            "A",
            "--shape",
            "B",
            "--value-stream",
            "Acme.VS",
            "--apply",
            "--json",
        ],
        &[
            "new",
            "thing-template",
            "Acme.TT",
            "--base",
            "GenericThing",
            "--shape",
            "A",
            "--visibility",
            "v",
        ],
        &[
            "new",
            "thing-shape",
            "Acme.TS",
            "--visibility",
            "v",
            "--json",
        ],
        &[
            "new",
            "data-shape",
            "Acme.DS",
            "--base",
            "Acme.Base",
            "--field",
            "id:STRING:primary",
            "--field",
            "n:NUMBER",
            "--visibility",
            "v",
        ],
        &[
            "new",
            "timer",
            "Acme.Timer",
            "--every",
            "60s",
            "--run-as",
            "Admin",
            "--disabled",
            "--then",
            "me.x = 1;",
            "--visibility",
            "v",
            "--project",
            "Acme",
        ],
        &[
            "new",
            "scheduler",
            "Acme.S",
            "--cron",
            "0 0 * * * ?",
            "--script",
            "run.js",
            "--run-as",
            "Admin",
        ],
        &[
            "new",
            "data-table",
            "Acme.DT",
            "--data-shape",
            "Acme.DS",
            "--index",
            "byName=name",
            "--index",
            "byId=id",
        ],
        &["new", "stream", "Acme.St", "--data-shape", "Acme.DS"],
        &["new", "value-stream", "Acme.VS", "--description", ""],
        &["new", "media", "Acme.M", "--file", "logo.png", "--apply"],
        &[
            "new",
            "style",
            "Acme.Style",
            "--background",
            "#fff",
            "--foreground",
            "#000",
            "--secondary-background",
            "#eee",
            "--line-color",
            "#111",
            "--line-style",
            "solid",
            "--line-thickness",
            "1",
            "--bold",
            "--italic",
            "--underline",
            "--text-size",
            "large",
            "--image",
            "Acme.M",
            "--display-string",
            "x",
        ],
        &[
            "new", "state", "Acme.SD", "--type", "string", "--state", "a", "--state", "b",
        ],
        &[
            "new",
            "menu",
            "Acme.Main_MN",
            "--item",
            "Home=Acme.Home_MU",
            "--item",
            "About=Acme.About_MU",
            "--default",
            "--label",
            "Main",
        ],
        &["new", "menu", "Acme.Main_MN", "--default", "x"],
        &["new", "menu", "Acme.Main_MN", "--default", "-1"],
        &[
            "new",
            "theme",
            "Acme.Theme",
            "--from",
            "Acme.Base",
            "--prop",
            "a=1",
            "--prop",
            "b=-2",
        ],
        &["new", "theme"],
        &["mashup", "validate", "Acme.Home_MU", "--json"],
        &["mashup", "validate", "--all"],
        &["mashup", "validate", "--all", "Acme.Home_MU"],
        &["mashup", "describe", "Acme.Home_MU", "--json"],
        &[
            "mashup",
            "add",
            "Acme.Home_MU",
            "ptcsbutton",
            "--right-of",
            "Title",
            "--label",
            "Go",
            "--prop",
            "Width=-1",
            "--prop",
            "Text=Go",
            "--input",
            "a:STRING",
            "--apply",
            "--json",
        ],
        &[
            "mashup",
            "add",
            "Acme.Home_MU",
            "ptcsgrid",
            "--in",
            "Main",
            "--above",
            "Title",
        ],
        &[
            "mashup",
            "add-contained",
            "Acme.Home_MU",
            "Acme.Child_MU",
            "--below",
            "Title",
            "--left-of",
            "Grid",
            "--set",
            "Count=1",
            "--set",
            "Name=x",
            "--apply",
        ],
        &[
            "mashup",
            "bind",
            "Acme.Home_MU",
            "--from",
            "Get.Result",
            "--to",
            "Grid.Data",
            "--json",
        ],
        &[
            "mashup",
            "on",
            "Acme.Home_MU",
            "--event",
            "Button.Clicked",
            "--invoke",
            "Get",
            "--apply",
        ],
        &[
            "mashup",
            "param",
            "add",
            "Acme.Home_MU",
            "Count",
            "--type",
            "INTEGER",
            "--direction",
            "IN",
            "--description",
            "-",
            "--default",
            "-1",
            "--data-shape",
            "Acme.DS",
            "--force",
            "--apply",
            "--json",
        ],
        &[
            "mashup",
            "param",
            "set",
            "Acme.Home_MU",
            "Count",
            "--default",
        ],
        &[
            "mashup",
            "param",
            "set",
            "Acme.Home_MU",
            "Count",
            "--no-default",
            "--default",
            "",
        ],
        &[
            "mashup",
            "recipe",
            "toast",
            "Acme.Home_MU",
            "Button",
            "--if",
            "x > 1",
            "--expr",
            "-x",
            "--input",
            "a:STRING",
            "--input",
            "b:NUMBER",
            "--after",
            "Get.Done",
            "--say",
            "hi",
            "--from",
            "Get",
            "--kind",
            "info",
            "--then",
            "Next",
            "--title",
            "T",
            "--on",
            "Button.Clicked",
            "--mashup",
            "Acme.Other_MU",
            "--window",
            "popup",
            "--size",
            "300x200",
        ],
        &[
            "mashup",
            "recipe",
            "grid",
            "Acme.Home_MU",
            "--pass",
            "a=Get.A",
            "--pass",
            "b=Get.B",
            "--reload",
            "Get",
            "--every",
            "5",
            "--interval",
            "Timer",
            "--else-say",
            "no",
            "--data",
            "Get.Result",
            "--x",
            "time",
            "--value",
            "v",
            "--column",
            "a:A:100",
            "--column",
            "b",
            "--selected",
            "Grid.Selected",
            "--apply",
            "--json",
        ],
        &[
            "mashup",
            "event",
            "add",
            "Acme.Home_MU",
            "Done",
            "--when",
            "Get.Done",
            "--apply",
            "--json",
        ],
        &["mashup", "templates"],
        &["mashup", "templates", "two-column", "--json"],
        &[
            "mashup",
            "new",
            "Acme.Home_MU",
            "--project",
            "Acme",
            "--template",
            "header",
            "--layout",
            "rows",
            "--primary-menu",
            "Acme.Main_MN",
            "--secondary-menu",
            "Acme.Side_MN",
            "--theme",
            "Acme.Theme",
            "--master",
            "--visibility",
            "Org:Acme",
            "--param",
            "Count:INTEGER",
            "--param",
            "Name:STRING:IN",
            "--tab",
            "Home=Acme.A_MU",
            "--tab",
            "About=Acme.B_MU",
            "--apply",
            "--json",
        ],
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
        let command = match path.split_once(' ') {
            Some((family, word)) => clap::Command::new(family)
                .subcommand(clap::Command::new(word).args(spec::arguments(flags, *operands))),
            None => clap::Command::new(path).args(spec::arguments(flags, *operands)),
        };
        let built = spec::decorate(clap::Command::new("twaco").subcommand(command));
        let old_matches = built
            .try_get_matches_from(&all)
            .unwrap_or_else(|e| panic!("{line:?}: {e}"));
        let (_, matched) = spec::matched(&old_matches);
        let typed = Typed::from_arg_matches(&matches).unwrap();
        // What the builder refused, validate() refuses in the same words.
        let old = match spec::args_from(flags, *operands, matched) {
            Ok(old) => old,
            Err(why) => {
                assert_eq!(typed.validate(), Err(why), "{line:?}");
                continue;
            }
        };
        assert_eq!(typed.validate(), Ok(()), "{line:?}");
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
        add("--backup", shown(old.backup.as_ref()));
        add("--restore", shown(old.restore.as_ref()));
        // `--prop` and `--set` both fill props; a command takes one of them.
        let props = if flags.contains(&"--set") {
            "--set"
        } else {
            "--prop"
        };
        for (name, repeated) in [
            ("--only", &old.only),
            ("--entity", &old.entity_filters),
            ("--visibility", &old.visibility),
            (props, &old.props),
            ("--param", &old.params),
            ("--tab", &old.tabs),
            ("--input", &old.inputs),
            ("--pass", &old.pass),
            ("--column", &old.columns),
            ("--shape", &old.shapes),
            ("--field", &old.fields),
            ("--index", &old.indexes),
            ("--state", &old.states),
            ("--item", &old.items),
        ] {
            for value in repeated {
                add(name, Some(value.clone()));
            }
        }
        let (names, mut on, mut given) = projection(typed);
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
