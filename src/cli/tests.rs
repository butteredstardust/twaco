use super::super::*;
use super::command::Command;
use super::data::Db;
use super::entity::entity_delete_force_deprecation;
use super::entity_new::{New, NewEntity};
use super::mashup::Mashup;
use super::spec::{self, USAGE};
use clap::FromArgMatches as _;

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
        .filter(|name| !spec::GLOBAL_FLAGS.iter().any(|(flag, _)| flag == name))
        .collect()
}

/// Parse a derived command's line as `twaco` does, up to what the command reads.
fn parse(words: &[&str]) -> Result<Command, String> {
    let mut all = vec!["twaco"];
    all.extend(words);
    let matches = spec::tree()
        .try_get_matches_from(all)
        .map_err(|e| e.to_string())?;
    let typed = Command::from_arg_matches(&matches).map_err(|e| e.to_string())?;
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
        "requirements",
        "contract",
        "mashup",
        "thing",
        "timer",
        "scheduler",
        "style",
        "media",
        "menu",
        "state",
        "network",
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
        let (found, _) = spec::matched(&matches);
        assert_eq!(found, command.path);
        let parsed = Command::from_arg_matches(&matches).unwrap();
        assert_eq!(parsed.path(), command.path);
        parsed.validate().unwrap();
    }
    spec::tree().debug_assert();
}

/// What a flag takes, as the command line sees it.
fn kind(arg: &clap::Arg) -> &'static str {
    let optional = arg
        .get_num_args()
        .is_some_and(|range| range.min_values() == 0);
    match arg.get_action() {
        clap::ArgAction::SetTrue => "switch",
        clap::ArgAction::Set if optional => "optional value",
        clap::ArgAction::Set => "value",
        clap::ArgAction::Append => "repeated",
        other => panic!("unexpected action {other:?}"),
    }
}

/// A flag means the same in every command that takes it: it takes the same kind of value, under
/// the same name (clap's errors quote it). Operands are `operands`.
#[test]
fn every_flag_means_the_same_in_every_command() {
    let tree = spec::tree();
    let mut seen: std::collections::BTreeMap<String, (&str, Vec<String>, &str)> =
        std::collections::BTreeMap::new();
    for command in USAGE {
        let node = node(&tree, command.path);
        let mut own = std::collections::BTreeSet::new();
        for arg in node.get_arguments() {
            let value_names: Vec<String> = arg
                .get_value_names()
                .unwrap_or_default()
                .iter()
                .map(ToString::to_string)
                .collect();
            // Operands, and before them a word that takes only the words it names: a sub-action
            // (`users list`) or a shell.
            if arg.is_positional() && arg.get_id() != "operands" {
                assert_eq!(value_names, [arg.get_id().as_str()], "{}", command.path);
                assert!(
                    !arg.get_possible_values().is_empty(),
                    "{}: an action names its words",
                    command.path
                );
                assert_eq!(
                    node.get_positionals().next().map(clap::Arg::get_id),
                    Some(arg.get_id()),
                    "{}: the action is the first word",
                    command.path
                );
                continue;
            }
            if arg.is_positional() {
                assert_eq!(arg.get_id(), "operands", "{}", command.path);
                assert_eq!(value_names, ["operand"], "{}", command.path);
                continue;
            }
            let name =
                flag_name(arg).unwrap_or_else(|| panic!("{}: an unnamed flag", command.path));
            assert!(own.insert(name.clone()), "{} {name} twice", command.path);
            let kind = kind(arg);
            assert_eq!(
                value_names.is_empty(),
                kind == "switch",
                "{} {name}: a value is named, a switch is not",
                command.path
            );
            let first =
                seen.entry(name.clone())
                    .or_insert((kind, value_names.clone(), command.path));
            assert_eq!(
                (first.0, &first.1),
                (kind, &value_names),
                "{name} takes one thing in {} and another in {}",
                first.2,
                command.path
            );
        }
    }
    for (flag, value) in spec::GLOBAL_FLAGS {
        assert_eq!(seen[*flag].1, [*value], "{flag}");
    }
}

/// The other way round from [`usage_names_every_flag_each_command_accepts`]: a flag a command's
/// help describes is one the command takes, so the text and the struct cannot drift apart.
#[test]
fn every_flag_a_command_describes_is_one_it_takes() {
    let mut absent = Vec::new();
    for command in USAGE {
        let takes = flags_of(command.path);
        let text = usage_of(command.path);
        let mut rest = text.as_str();
        while let Some(at) = rest.find("--") {
            let after = &rest[at..];
            let end = after[2..]
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
                .map_or(after.len(), |end| end + 2);
            let flag = &after[..end];
            let starts_word = rest[..at]
                .chars()
                .next_back()
                .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '-'));
            // `mashup on --event` names another command's flag.
            let elsewhere = USAGE
                .iter()
                .any(|other| rest[..at].ends_with(&format!("{} ", other.path)));
            if starts_word && !elsewhere && flag.len() > 2 && !takes.iter().any(|own| own == flag) {
                absent.push(format!("{} {flag}", command.path));
            }
            rest = &after[end.max(2)..];
        }
    }
    assert!(
        absent.is_empty(),
        "flags described but not taken: {absent:?}"
    );
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
    let Command::Deploy(deploy) = parse(&[
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
    .unwrap() else {
        panic!("deploy parses as deploy");
    };
    assert_eq!(deploy.only, ["A", "B"]);
    assert_eq!(deploy.only_projects.projects(), ["P", "Q"]);
    assert_eq!(deploy.profile.name(), "x");
    let Command::Logs(logs) = parse(&["logs", "ScriptLog", "--since", "10m", "--json"]).unwrap()
    else {
        panic!("logs parses as logs");
    };
    assert_eq!(logs.operands, ["ScriptLog"]);
    assert_eq!(logs.since.as_deref(), Some("10m"));
    assert!(logs.json.json);
    let Command::Init(init) =
        parse(&["init", "--from-server", "A", "B", "--apply", "--json"]).unwrap()
    else {
        panic!("init parses as init");
    };
    assert_eq!(init.from_server, ["A", "B"]);
    assert!(init.apply.apply && init.json.json);
    let Command::Db(Db::Query(query)) =
        parse(&["db", "query", "-q", "select 1", "--timeout", "30"]).unwrap()
    else {
        panic!("db query parses as db query");
    };
    assert_eq!(query.sql.as_deref(), Some("select 1"));
    assert_eq!(query.timeout.timeout, Some(Duration::from_secs(30)));
    // clap refuses a timeout or a project list that does not read, before the command runs.
    for (line, refusal) in [
        (
            &["call", "T", "S", "--timeout", "0"][..],
            "invalid value '0' for '--timeout <seconds>': needs a positive whole number of seconds",
        ),
        (
            &["db", "query", "q.sql", "--timeout", "soon"][..],
            "invalid value 'soon' for '--timeout <seconds>': needs a positive whole number of seconds",
        ),
        (
            &["deploy", "--only-projects", " , "][..],
            "invalid value ' , ' for '--only-projects <a,b>': needs at least one project",
        ),
    ] {
        let error = spec::tree().try_get_matches_from(
            std::iter::once("twaco").chain(line.iter().copied()),
        );
        let error = error.unwrap_err();
        assert_eq!(error.exit_code(), 2, "{line:?}");
        assert!(error.to_string().contains(refusal), "{line:?}: {error}");
    }
    let Command::Call(call) = parse(&["call", "T", "S", "--timeout", "30"]).unwrap() else {
        panic!("call parses as call");
    };
    assert_eq!(
        call.timeout.or(Duration::from_secs(120)),
        Duration::from_secs(30)
    );
    let Command::Call(call) = parse(&["call", "T", "S"]).unwrap() else {
        panic!("call parses as call");
    };
    assert_eq!(
        call.timeout.or(Duration::from_secs(120)),
        Duration::from_secs(120)
    );
    assert_eq!(
        parse(&["sync", "--all", "Acme.T"]).map(|_| ()),
        Err("--all and [\"Acme.T\"] say different things; pass one or the other".to_string())
    );
    // A value may begin with a dash, as a log filter can.
    let Command::Logs(grep) = parse(&["logs", "ScriptLog", "--grep", "-x"]).unwrap() else {
        panic!("logs parses as logs");
    };
    assert_eq!(grep.grep.as_deref(), Some("-x"));
    // A negative number is an operand; a mistyped flag after operands is still refused.
    let Command::Call(call) = parse(&["call", "T", "S", "-5"]).unwrap() else {
        panic!("call parses as call");
    };
    assert_eq!(call.operands, ["T", "S", "-5"]);
    assert!(parse(&["sync", "Acme.T", "--cehck"]).is_err());
    // A file named with a leading dash goes after `--`.
    let Command::Db(Db::Run(file)) = parse(&["db", "run", "--", "-migration.sql"]).unwrap() else {
        panic!("db run parses as db run");
    };
    assert_eq!(file.operands, ["-migration.sql"]);
    // A switch said twice is said once; a value given twice is refused, as before.
    let Command::Deploy(deploy) = parse(&["deploy", "--force", "--force"]).unwrap() else {
        panic!("deploy parses as deploy");
    };
    assert!(deploy.force);
    assert!(parse(&["deploy", "--profile", "a", "--profile", "b"]).is_err());
    // `--flag=value` works too.
    let Command::Deploy(deploy) = parse(&["deploy", "--profile=prod"]).unwrap() else {
        panic!("deploy parses as deploy");
    };
    assert_eq!(deploy.profile.name(), "prod");
    let Command::Doctor(doctor) = parse(&["doctor", "--profile=prod"]).unwrap() else {
        panic!("doctor parses as doctor");
    };
    assert_eq!(doctor.profile.name(), "prod");
    let Command::Doctor(doctor) = parse(&["doctor"]).unwrap() else {
        panic!("doctor parses as doctor");
    };
    assert_eq!(doctor.profile.name(), "default");
    assert!(parse(&["doctor", "--profile", "a", "--profile", "b"]).is_err());
    let Command::Update(update) = parse(&["update", "--apply", "--apply"]).unwrap() else {
        panic!("update parses as update");
    };
    assert!(update.apply.apply);
    let Command::Mcp(mcp) = parse(&["mcp", "--tools", "-core"]).unwrap() else {
        panic!("mcp parses as mcp");
    };
    assert_eq!(mcp.tools.as_deref(), Some("-core"));
    // An optional value given bare is followed by the next flag, never takes it as its value.
    let Command::New(New::Entity(NewEntity::Menu(menu))) = parse(&[
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
    let Command::Mashup(Mashup::Param(param)) = parse(&[
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
    // A sub-action is read into its own field, and the operands follow it.
    assert_eq!(param.action, Some(super::mashup::ParamVerb::Set));
    assert_eq!(param.operands, ["Acme.Home_MU", "Count"]);
    let Command::Users(users) = parse(&["users", "seed", "--apply"]).unwrap() else {
        panic!("users parses as users");
    };
    assert_eq!(users.action, Some(super::users::UsersVerb::Seed));
    let Command::Help(help) = parse(&["help", "search", "mashup", "binding"]).unwrap() else {
        panic!("help parses as help");
    };
    assert_eq!(help.action, Some(super::info::HelpVerb::Search));
    assert_eq!(help.operands, ["mashup", "binding"]);
    let Command::Javadoc(javadoc) = parse(&["javadoc", "class", "Thing"]).unwrap() else {
        panic!("javadoc parses as javadoc");
    };
    assert_eq!(javadoc.action, Some(super::info::JavadocVerb::Class));
    assert!(parse(&["users", "list", "extra"]).is_err());
    assert!(parse(&["javadoc", "frob"]).is_err());
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

/// Shell completions describe each command and each flag that has a line of its own in the help
/// text, in the help text's words, and offer a sub-action's words.
#[test]
fn completions_describe_commands_and_flags_from_the_help_text() {
    let tree = spec::tree();
    let help = |path: &str, flag: &str| {
        node(&tree, path)
            .get_arguments()
            .find(|arg| flag_name(arg).as_deref() == Some(flag))
            .and_then(|arg| arg.get_help().map(ToString::to_string))
    };
    assert_eq!(
        help("sync", "--check").as_deref(),
        Some("report what would change, write nothing")
    );
    // A description continued on the lines below is one line.
    assert_eq!(
        help("mcp", "--tools").as_deref(),
        Some(
            "only these tool groups, comma-separated (TWACO_MCP_TOOLS): core, entity, refactor, mashup, permissions, data, server, reference; default all"
        )
    );
    // The shared flags are described where the listing describes them.
    assert_eq!(
        help("doctor", "--profile").as_deref(),
        Some("the server profile (default: default)")
    );
    assert_eq!(
        help("check", "--log").as_deref(),
        Some("write diagnostic logs to stderr (or TWACO_LOG)")
    );
    // Several flags on one line share it; a flag named only in a synopsis has no description.
    assert_eq!(help("logs", "--thread").as_deref(), Some("exact matches"));
    assert_eq!(help("init", "--write"), None);
    // `--only` is not `--only-projects`.
    assert_ne!(help("deploy", "--only"), help("deploy", "--only-projects"));
    for command in USAGE {
        let about = node(&tree, command.path)
            .get_about()
            .map(ToString::to_string);
        let first = command.text.lines().next().unwrap_or_default();
        // A command whose synopsis line says what it does, or whose next line does, has it.
        let says = first.trim().contains("  ")
            || command
                .text
                .lines()
                .nth(1)
                .is_some_and(|line| line.starts_with(&" ".repeat(30)));
        assert_eq!(
            about.is_some_and(|about| !about.is_empty()),
            says,
            "{}: its description",
            command.path
        );
    }
    assert_eq!(
        node(&tree, "sync").get_about().map(ToString::to_string),
        Some("sidecars -> entity XML".to_string())
    );
    for shell in [
        clap_complete::Shell::Bash,
        clap_complete::Shell::Elvish,
        clap_complete::Shell::Fish,
        clap_complete::Shell::PowerShell,
        clap_complete::Shell::Zsh,
    ] {
        let mut script = Vec::new();
        clap_complete::generate(shell, &mut spec::tree(), "twaco", &mut script);
        let script = String::from_utf8(script).unwrap();
        for word in ["permissions", "only-projects"] {
            assert!(script.contains(word), "{shell}: {word}");
        }
        // Bash and zsh offer an action's words too; the others complete commands and flags.
        let values = matches!(
            shell,
            clap_complete::Shell::Bash | clap_complete::Shell::Zsh
        );
        assert_eq!(script.contains("seed"), values, "{shell}: seed");
        if shell != clap_complete::Shell::Bash {
            assert!(
                script.contains("report what would change, write nothing"),
                "{shell} describes flags"
            );
        }
    }
    let Command::Completions(completions) = parse(&["completions", "zsh"]).unwrap() else {
        panic!("completions parses as completions");
    };
    assert_eq!(completions.shell, Some(clap_complete::Shell::Zsh));
    assert!(parse(&["completions", "tcsh"]).is_err());
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
    // A value the flag takes: any word, or a number where it takes a number (`--timeout`).
    let value = ["v", "1"]
        .into_iter()
        .find(|value| {
            let argv: Vec<&str> = std::iter::once("twaco")
                .chain(words.iter().copied())
                .chain([name.as_str(), value])
                .collect();
            !matches!(
                probe.try_get_matches_from_mut(argv),
                Err(error) if error.kind() == clap::error::ErrorKind::ValueValidation
            )
        })
        .unwrap_or("v");
    let given = |times: usize| -> Vec<String> {
        let mut argv: Vec<String> = std::iter::once("twaco")
            .chain(words.iter().copied())
            .map(str::to_string)
            .collect();
        for _ in 0..times {
            argv.push(name.clone());
            if takes_value {
                argv.push(value.to_string());
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

#[test]
fn a_blank_tools_variable_serves_every_group() {
    assert_eq!(crate::tool_groups(None, Some(String::new())), None);
    assert_eq!(crate::tool_groups(None, Some("  ".to_string())), None);
    assert_eq!(
        crate::tool_groups(None, Some("read".to_string())).as_deref(),
        Some("read")
    );
    assert_eq!(
        crate::tool_groups(Some("edit"), Some("read".to_string())).as_deref(),
        Some("edit")
    );
}
