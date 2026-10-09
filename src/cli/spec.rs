//! Every command, once: its path, what it says about itself, the flags it takes and whether it
//! takes operands. clap parses against it (unknown flags and operands are refused, with a
//! suggestion), `twaco` with no arguments and `documentation/COMMANDS.md` list it, and
//! [`Args`](super::args::Args) is filled from what clap matched.
//!
//! A flag means the same in every command that takes it, so its kind is declared once in
//! [`FLAGS`]: a switch, a value, or a value that may repeat.

use super::args::Args;
use std::time::Duration;

/// What a flag takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Takes {
    Switch,
    /// One value, named as help shows it.
    Value(&'static str),
    /// A value, and the flag may be given again.
    Repeated(&'static str),
}

#[derive(Debug)]
pub(crate) struct Flag {
    /// `--name`, or `-q` for the one short flag.
    pub(crate) name: &'static str,
    pub(crate) takes: Takes,
}

#[derive(Debug)]
pub(crate) struct Command {
    /// One word, or two for a family (`entity push`).
    pub(crate) path: &'static str,
    /// Its section in [`GROUPS`].
    pub(crate) group: usize,
    /// Whether it takes positional operands (entity names, files, sub-actions).
    pub(crate) operands: bool,
    pub(crate) flags: &'static [&'static str],
    /// What `twaco` with no arguments shows for it: synopsis lines at two spaces, flag lines at
    /// six, descriptions from column 31. A test holds it to every flag in `flags`.
    pub(crate) text: &'static str,
}

/// The flag's kind; every name a command lists is declared here.
pub(crate) fn flag(name: &str) -> &'static Flag {
    FLAGS
        .iter()
        .find(|flag| flag.name == name)
        .unwrap_or_else(|| panic!("{name} is not declared in FLAGS"))
}

/// The command a path names.
pub(crate) fn command(path: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|command| command.path == path)
}

/// The clap id of a flag: its name without dashes.
fn id(name: &str) -> &str {
    name.trim_start_matches('-')
}

/// Flags every command accepts without listing them: diagnostic logs. See `core::diagnostics`.
pub(crate) const GLOBAL_FLAGS: &[&str] = &["--log", "--log-file"];

/// The diagnostic options a parse matched: `--log` and `--log-file`.
pub(crate) fn log_options(matches: &clap::ArgMatches) -> (Option<String>, Option<String>) {
    let get = |name: &str| matches.get_one::<String>(id(name)).cloned();
    (get("--log"), get("--log-file"))
}

/// The first synopsis of a command, as clap shows it in an error.
fn synopsis(command: &Command) -> String {
    let first = command.text.lines().next().unwrap_or_default().trim();
    let synopsis = first.split("  ").next().unwrap_or(first).trim();
    format!("twaco {synopsis}")
}

fn leaf(word: &'static str, command: &'static Command) -> clap::Command {
    clap::Command::new(word)
        .override_usage(synopsis(command))
        .override_help(help_of(command))
        .disable_version_flag(true)
        .args(arguments(command.flags, command.operands))
        .args(arguments(GLOBAL_FLAGS, false))
}

/// The clap arguments of a command that takes these flags, and operands or not.
pub(crate) fn arguments(flags: &[&str], operands: bool) -> Vec<clap::Arg> {
    let mut built = Vec::new();
    for name in flags {
        let declared = flag(name);
        let name = declared.name;
        let mut arg = clap::Arg::new(id(name));
        arg = match name.strip_prefix("--") {
            Some(long) => arg.long(long),
            None => arg.short(name.chars().nth(1).expect("a short flag has a letter")),
        };
        arg = match declared.takes {
            // Saying a switch twice says it once, as it always did.
            Takes::Switch => arg
                .action(clap::ArgAction::SetTrue)
                .overrides_with(id(name)),
            Takes::Value(value) => arg
                .value_name(value)
                .action(clap::ArgAction::Set)
                .allow_hyphen_values(true),
            Takes::Repeated(value) => arg
                .value_name(value)
                .action(clap::ArgAction::Append)
                .allow_hyphen_values(true),
        };
        built.push(arg);
    }
    if operands {
        built.push(
            // A negative number is an operand (a JSON value for `call`); any other word that
            // starts with a dash is a flag, so a mistyped one is refused, not taken as a name.
            // A file whose name starts with a dash goes after `--`.
            clap::Arg::new("operands")
                .num_args(0..)
                .action(clap::ArgAction::Append)
                .allow_negative_numbers(true)
                .value_name("operand"),
        );
    }
    built
}

/// The whole command line: every command, with families as nested subcommands.
pub(crate) fn tree() -> clap::Command {
    let mut root = clap::Command::new("twaco")
        // Built once per run; clap keeps the version as a static string.
        .version(&*Box::leak(twaco::version().into_boxed_str()))
        .override_usage("twaco <command>")
        .override_help(listing())
        .disable_help_subcommand(true)
        .subcommand_required(true)
        .arg_required_else_help(true);
    // A family (`entity`, `rename`...) is a command of its own whose commands are the words
    // after it; it is listed where its first command is.
    let mut families: Vec<&'static str> = Vec::new();
    for command in COMMANDS {
        match command.path.split_once(' ') {
            None => root = root.subcommand(leaf(command.path, command)),
            Some((family, _)) if !families.contains(&family) => {
                let family: &'static str = &command.path[..family.len()];
                families.push(family);
                let mut built = clap::Command::new(family)
                    .override_usage(format!("twaco {family} <command>"))
                    .override_help(family_listing(family))
                    .subcommand_required(true)
                    .arg_required_else_help(true)
                    .disable_help_subcommand(true)
                    .disable_version_flag(true);
                for member in COMMANDS {
                    if member.path.split_once(' ').map(|(name, _)| name) == Some(family) {
                        built = built.subcommand(leaf(&member.path[family.len() + 1..], member));
                    }
                }
                root = root.subcommand(built);
            }
            Some(_) => {}
        }
    }
    root
}

/// The command a parse matched, and the matches of that command.
pub(crate) fn matched(matches: &clap::ArgMatches) -> (&'static Command, &clap::ArgMatches) {
    let (first, sub) = matches.subcommand().expect("a subcommand is required");
    if let Some(command) = command(first) {
        return (command, sub);
    }
    let (second, leaf) = sub.subcommand().expect("a family requires a command");
    let command = COMMANDS
        .iter()
        .find(|c| c.path.split_once(' ') == Some((first, second)))
        .expect("clap matched only commands in COMMANDS");
    (command, leaf)
}

/// What the command matched, as the commands read it.
pub(crate) fn args_of(command: &Command, matches: &clap::ArgMatches) -> Result<Args, String> {
    args_from(command.flags, command.operands, matches)
}

/// What a parse with [`arguments`] matched, as the commands read it.
pub(crate) fn args_from(
    flags: &[&str],
    operands: bool,
    matches: &clap::ArgMatches,
) -> Result<Args, String> {
    let mut args = Args::default();
    if operands {
        args.names = matches
            .get_many::<String>("operands")
            .map(|values| values.cloned().collect())
            .unwrap_or_default();
    }
    for name in flags {
        let key = id(name);
        match flag(name).takes {
            Takes::Switch => {
                if matches.get_flag(key) {
                    args.flags.push(name.to_string());
                }
            }
            Takes::Repeated(_) => {
                let values: Vec<String> = matches
                    .get_many::<String>(key)
                    .map(|values| values.cloned().collect())
                    .unwrap_or_default();
                match *name {
                    "--only" => args.only.extend(values),
                    "--entity" => args.entity_filters.extend(values),
                    other => unreachable!("{other} repeats but nothing reads it"),
                }
            }
            Takes::Value(_) => {
                let Some(value) = matches.get_one::<String>(key).cloned() else {
                    continue;
                };
                match *name {
                    "--project" => args.project = Some(value),
                    "--profile" => args.profile = Some(value),
                    "--out" => args.out = Some(value.into()),
                    "--backup" => args.backup = Some(value.into()),
                    "--restore" => args.restore = Some(value.into()),
                    "--only-projects" => {
                        let selected: Vec<String> = value
                            .split(',')
                            .map(str::trim)
                            .filter(|name| !name.is_empty())
                            .map(str::to_string)
                            .collect();
                        if selected.is_empty() {
                            return Err("--only-projects needs at least one project".to_string());
                        }
                        args.only_projects.extend(selected);
                    }
                    "--timeout" => {
                        let seconds = value
                            .parse::<u64>()
                            .ok()
                            .filter(|seconds| *seconds > 0)
                            .ok_or_else(|| {
                                "--timeout needs a positive whole number of seconds".to_string()
                            })?;
                        args.timeout = Some(Duration::from_secs(seconds));
                    }
                    other => {
                        args.values.insert(other.to_string(), value);
                    }
                }
            }
        }
    }
    if args.has("--all") && !args.names.is_empty() {
        return Err(format!(
            "--all and {:?} say different things; pass one or the other",
            args.names
        ));
    }
    Ok(args)
}

/// Flags every command describes once, at the end of the listing.
const SHARED: &str = "  --project <name>            narrow to one project of the solution\n  --profile <name>            the server profile (default: default)";

/// The diagnostic flags: any command takes them, so every command's help ends with them.
const LOGGING: &str = "  --log <filter>              write diagnostic logs to stderr (or TWACO_LOG)\n  --log-file <path>           append diagnostic logs to a file (or TWACO_LOG_FILE)";

/// `twaco <command> --help`: its block, then the shared flags it takes.
fn help_of(command: &Command) -> String {
    let shared: Vec<&str> = SHARED
        .lines()
        .filter(|line| {
            let name = line.split_whitespace().next().unwrap_or_default();
            command.flags.contains(&name)
        })
        .collect();
    let shared = if shared.is_empty() {
        LOGGING.to_string()
    } else {
        format!("{}\n{LOGGING}", shared.join("\n"))
    };
    format!("{}\n\n{shared}\n", command.text)
}

/// `twaco` with no arguments: every command, in its group's order, then the shared flags.
pub(crate) fn listing() -> String {
    let mut out = String::from("usage: twaco <command>\n\n");
    for group in 0..GROUPS.len() {
        for command in COMMANDS.iter().filter(|c| c.group == group) {
            out.push_str(command.text);
            out.push('\n');
        }
    }
    out.push('\n');
    out.push_str(FOOTER);
    out.push('\n');
    out
}

/// One family's commands, for `twaco entity` and `twaco entity --help`.
fn family_listing(family: &str) -> String {
    let mut out = format!("usage: twaco {family} <command>\n\n");
    for command in COMMANDS
        .iter()
        .filter(|c| c.path.split_once(' ').is_some_and(|(f, _)| f == family))
    {
        out.push_str(command.text);
        out.push('\n');
    }
    out.push('\n');
    out.push_str(SHARED);
    out.push('\n');
    out.push_str(LOGGING);
    out.push('\n');
    out
}

/// `documentation/COMMANDS.md`, from the same table; a test keeps the file equal to it.
#[cfg(test)]
pub(crate) fn commands_md() -> String {
    let mut out = String::from(
        "# Commands\n\nEvery twaco command and flag, from `twaco` run with no arguments. Commands that change a server\nprint a plan unless given `--apply`; `call` is the exception, because twaco cannot tell whether a\nservice writes. A server command takes `--profile <name>` (default: `default`); see\n[Configuration](CONFIGURATION.md#server-profiles). `twaco <command> --help` shows one command.\n\n",
    );
    for (group, (title, intro)) in GROUPS.iter().enumerate() {
        out.push_str(&format!("## {title}\n\n{intro}\n\n```text\n"));
        for command in COMMANDS.iter().filter(|c| c.group == group) {
            out.push_str(&dedent(command.text));
        }
        out.push_str("```\n\n");
    }
    out.push_str("## Shared\n\nFlags described once for the commands that list them, and exit codes.\n\n```text\n");
    out.push_str(&dedent(FOOTER));
    out.push_str("```\n");
    out
}

/// Text without the two spaces the listing indents it by, a line end after each line.
#[cfg(test)]
fn dedent(text: &str) -> String {
    text.lines()
        .map(|line| format!("{}\n", line.strip_prefix("  ").unwrap_or(line)))
        .collect()
}

/// Every flag, and what it takes.
pub(crate) const FLAGS: &[Flag] = &[
    Flag {
        name: "--accept-loss",
        takes: Takes::Switch,
    },
    Flag {
        name: "--add-shapes",
        takes: Takes::Value("value"),
    },
    Flag {
        name: "--agents",
        takes: Takes::Switch,
    },
    Flag {
        name: "--all",
        takes: Takes::Switch,
    },
    Flag {
        name: "--allow-add-remove",
        takes: Takes::Switch,
    },
    Flag {
        name: "--allow-file-repository-data-loss",
        takes: Takes::Switch,
    },
    Flag {
        name: "--allow-outside-dependents",
        takes: Takes::Switch,
    },
    Flag {
        name: "--allow-repository-defined",
        takes: Takes::Switch,
    },
    Flag {
        name: "--append",
        takes: Takes::Switch,
    },
    Flag {
        name: "--apply",
        takes: Takes::Switch,
    },
    Flag {
        name: "--as",
        takes: Takes::Value("new"),
    },
    Flag {
        name: "--backend-only",
        takes: Takes::Switch,
    },
    Flag {
        name: "--base",
        takes: Takes::Value("file|handoff|git rev"),
    },
    Flag {
        name: "--backup",
        takes: Takes::Value("file"),
    },
    Flag {
        name: "--base-extension",
        takes: Takes::Value("value"),
    },
    Flag {
        name: "--check",
        takes: Takes::Switch,
    },
    Flag {
        name: "--collection",
        takes: Takes::Value("name"),
    },
    Flag {
        name: "--depth",
        takes: Takes::Value("n"),
    },
    Flag {
        name: "--description",
        takes: Takes::Value("text"),
    },
    Flag {
        name: "--detail",
        takes: Takes::Switch,
    },
    Flag {
        name: "--diff",
        takes: Takes::Switch,
    },
    Flag {
        name: "--display-name",
        takes: Takes::Value("text"),
    },
    Flag {
        name: "--dot",
        takes: Takes::Switch,
    },
    Flag {
        name: "--drop-unmapped",
        takes: Takes::Switch,
    },
    Flag {
        name: "--editable",
        takes: Takes::Switch,
    },
    Flag {
        name: "--entity",
        takes: Takes::Repeated("name"),
    },
    Flag {
        name: "--fail-on-revert",
        takes: Takes::Switch,
    },
    Flag {
        name: "--force",
        takes: Takes::Switch,
    },
    Flag {
        name: "--from",
        takes: Takes::Value("time"),
    },
    Flag {
        name: "--from-helper",
        takes: Takes::Switch,
    },
    Flag {
        name: "--frontend-only",
        takes: Takes::Switch,
    },
    Flag {
        name: "--grep",
        takes: Takes::Value("text"),
    },
    Flag {
        name: "--json",
        takes: Takes::Switch,
    },
    Flag {
        name: "--handoff",
        takes: Takes::Value("name"),
    },
    Flag {
        name: "--leave-delegate",
        takes: Takes::Switch,
    },
    Flag {
        name: "--level",
        takes: Takes::Value("level"),
    },
    Flag {
        name: "--language-common",
        takes: Takes::Value("text"),
    },
    Flag {
        name: "--language-native",
        takes: Takes::Value("text"),
    },
    Flag {
        name: "--limit",
        takes: Takes::Value("n"),
    },
    Flag {
        name: "--log",
        takes: Takes::Value("filter"),
    },
    Flag {
        name: "--log-file",
        takes: Takes::Value("path"),
    },
    Flag {
        name: "--live",
        takes: Takes::Switch,
    },
    Flag {
        name: "--map",
        takes: Takes::Value("value"),
    },
    Flag {
        name: "--max-rows",
        takes: Takes::Value("n"),
    },
    Flag {
        name: "--member",
        takes: Takes::Value("name"),
    },
    Flag {
        name: "--min-confidence",
        takes: Takes::Value("value"),
    },
    Flag {
        name: "--model-logic",
        takes: Takes::Switch,
    },
    Flag {
        name: "--no-backup",
        takes: Takes::Switch,
    },
    Flag {
        name: "--no-management-shape",
        takes: Takes::Switch,
    },
    Flag {
        name: "--no-sql",
        takes: Takes::Switch,
    },
    Flag {
        name: "--no-transaction",
        takes: Takes::Switch,
    },
    Flag {
        name: "--name",
        takes: Takes::Value("name"),
    },
    Flag {
        name: "--oldest-first",
        takes: Takes::Switch,
    },
    Flag {
        name: "--only",
        takes: Takes::Repeated("entity"),
    },
    Flag {
        name: "--take",
        takes: Takes::Value("side:name,..."),
    },
    Flag {
        name: "--only-projects",
        takes: Takes::Value("a,b"),
    },
    Flag {
        name: "--origin",
        takes: Takes::Value("value"),
    },
    Flag {
        name: "--out",
        takes: Takes::Value("path"),
    },
    Flag {
        name: "--overwrite",
        takes: Takes::Switch,
    },
    Flag {
        name: "--overwrite-properties",
        takes: Takes::Switch,
    },
    Flag {
        name: "--overwrite-tables",
        takes: Takes::Switch,
    },
    Flag {
        name: "--parent",
        takes: Takes::Value("block"),
    },
    Flag {
        name: "--path",
        takes: Takes::Value("path"),
    },
    Flag {
        name: "--prune",
        takes: Takes::Switch,
    },
    Flag {
        name: "--platform",
        takes: Takes::Switch,
    },
    Flag {
        name: "--profile",
        takes: Takes::Value("name"),
    },
    Flag {
        name: "--project",
        takes: Takes::Value("name"),
    },
    Flag {
        name: "--record",
        takes: Takes::Switch,
    },
    Flag {
        name: "--recursive",
        takes: Takes::Switch,
    },
    Flag {
        name: "--refresh",
        takes: Takes::Switch,
    },
    Flag {
        name: "--regex",
        takes: Takes::Value("re"),
    },
    Flag {
        name: "--relayout",
        takes: Takes::Switch,
    },
    Flag {
        name: "--remove-shapes",
        takes: Takes::Value("value"),
    },
    Flag {
        name: "--renamed",
        takes: Takes::Switch,
    },
    Flag {
        name: "--repository",
        takes: Takes::Value("repository"),
    },
    Flag {
        name: "--reset",
        takes: Takes::Switch,
    },
    Flag {
        name: "--restore",
        takes: Takes::Value("file"),
    },
    Flag {
        name: "--root",
        takes: Takes::Value("dir"),
    },
    Flag {
        name: "--search",
        takes: Takes::Value("text"),
    },
    Flag {
        name: "--section",
        takes: Takes::Value("heading"),
    },
    Flag {
        name: "--server",
        takes: Takes::Switch,
    },
    Flag {
        name: "--since",
        takes: Takes::Value("duration"),
    },
    Flag {
        name: "--skip-checks",
        takes: Takes::Switch,
    },
    Flag {
        name: "--sql",
        takes: Takes::Switch,
    },
    Flag {
        name: "--sql-dir",
        takes: Takes::Value("dir"),
    },
    Flag {
        name: "--sublogger",
        takes: Takes::Value("name"),
    },
    Flag {
        name: "--tags",
        takes: Takes::Value("value"),
    },
    Flag {
        name: "--table",
        takes: Takes::Value("name"),
    },
    Flag {
        name: "--text",
        takes: Takes::Switch,
    },
    Flag {
        name: "--thing",
        takes: Takes::Value("name"),
    },
    Flag {
        name: "--thread",
        takes: Takes::Value("value"),
    },
    Flag {
        name: "--timeout",
        takes: Takes::Value("seconds"),
    },
    Flag {
        name: "--to",
        takes: Takes::Value("value"),
    },
    Flag {
        name: "--type",
        takes: Takes::Value("value"),
    },
    Flag {
        name: "--user",
        takes: Takes::Value("value"),
    },
    Flag {
        name: "--usage",
        takes: Takes::Value("text"),
    },
    Flag {
        name: "--context",
        takes: Takes::Value("text"),
    },
    Flag {
        name: "--value",
        takes: Takes::Value("text"),
    },
    Flag {
        name: "--version",
        takes: Takes::Value("release"),
    },
    Flag {
        name: "--with-dependents",
        takes: Takes::Switch,
    },
    Flag {
        name: "--with-logs",
        takes: Takes::Switch,
    },
    Flag {
        name: "--write",
        takes: Takes::Switch,
    },
    Flag {
        name: "--zip",
        takes: Takes::Value("name"),
    },
    Flag {
        name: "-q",
        takes: Takes::Value("sql"),
    },
];

/// Every command, in the order `twaco` lists them within a group.
pub(crate) const COMMANDS: &[Command] = &[
    Command {
        path: "init",
        group: 0,
        operands: false,
        flags: &["--write", "--agents"],
        text: r#"  init [--write]              propose a twaco.toml from the repository's own entities; --write
                              also writes AGENTS.md and CLAUDE.md where absent, and adds the
                              lines twaco needs to .gitignore
  init --agents               write only those, where absent"#,
    },
    Command {
        path: "doctor",
        group: 0,
        operands: false,
        flags: &["--profile"],
        text: r#"  doctor [--profile <name>]   what resolved, what is reachable, what is missing"#,
    },
    Command {
        path: "projects",
        group: 0,
        operands: false,
        flags: &[],
        text: r#"  projects                    the solution's projects and their deploy order"#,
    },
    Command {
        path: "mcp",
        group: 0,
        operands: false,
        flags: &[],
        text: r#"  mcp                         serve the tools over MCP on stdio (TWACO_ROOT: the solution)"#,
    },
    Command {
        path: "update",
        group: 0,
        operands: false,
        flags: &["--apply"],
        text: r#"  update [--apply]            compare with the latest release; --apply verifies its signature
                              and replaces this binary (TWACO_NO_UPDATE_CHECK=1: no daily notice)"#,
    },
    Command {
        path: "extract",
        group: 1,
        operands: true,
        flags: &["--all", "--project"],
        text: r#"  extract <entity>|--all      entity XML -> sidecars"#,
    },
    Command {
        path: "sync",
        group: 1,
        operands: true,
        flags: &[
            "--all",
            "--check",
            "--allow-add-remove",
            "--relayout",
            "--project",
        ],
        text: r#"  sync <entity>|--all         sidecars -> entity XML
      --check                 report what would change, write nothing
      --allow-add-remove      add and remove services and fields as the sidecars do
      --relayout              rewrite scripts in the configured CDATA layout"#,
    },
    Command {
        path: "fmt",
        group: 1,
        operands: false,
        flags: &["--check"],
        text: r#"  fmt [--check]               format service scripts"#,
    },
    Command {
        path: "check",
        group: 1,
        operands: false,
        flags: &["--detail", "--live", "--profile"],
        text: r#"  check [--detail]            every gate, one exit code
      --live                  also parse every script on the server (fails closed)"#,
    },
    Command {
        path: "types",
        group: 1,
        operands: false,
        flags: &["--platform", "--check", "--json", "--profile"],
        text: r#"  types [--check [--json]|--platform] [--profile <name>]
                              generate declarations; --check runs TypeScript once
                              suppress one finding on the line above with
                              // @ts-ignore or // @ts-expect-error
                              [[check]] hook: command = ["twaco", "types", "--check", "--json"]"#,
    },
    Command {
        path: "catalog",
        group: 1,
        operands: true,
        flags: &["--project", "--search", "--json"],
        text: r#"  catalog [<entity>] [--project P] [--search <text>] [--json]
                              offline services, signatures, origins and descriptions"#,
    },
    Command {
        path: "impact",
        group: 1,
        operands: true,
        flags: &[
            "--member",
            "--min-confidence",
            "--depth",
            "--detail",
            "--json",
            "--dot",
        ],
        text: r#"  impact <entity> [--member <name>] [--min-confidence structural|resolved|review] [--depth <n>]
                              what changing an entity, or one service/property/field of it, reaches
      --detail                list every dependent and the chain of references to each
      --json                  the report as JSON (chains with --detail)
      --dot                   the dependents as a Graphviz graph"#,
    },
    Command {
        path: "unused",
        group: 1,
        operands: false,
        flags: &["--min-confidence", "--collection", "--detail", "--json"],
        text: r#"  unused [--min-confidence structural|resolved|review] [--collection <name>]
                              entities no entry point reaches; advisory, deletes nothing
      --detail                list every one, with the entry points and each file
      --json                  the report as JSON"#,
    },
    Command {
        path: "docs",
        group: 1,
        operands: false,
        flags: &["--detail", "--json", "--out", "--force"],
        text: r#"  docs [--out <file>] [--force]
                              the solution written down: projects and deploy order, inheritance,
                              services, DataShapes, and the references to review; Markdown
      --detail                every signature and field, and every review reference
      --json                  JSON instead of Markdown
      --out <file>            write it to one file (atomically) instead of printing; --force replaces"#,
    },
    Command {
        path: "adopt",
        group: 1,
        operands: true,
        flags: &[
            "--entity",
            "--base",
            "--only",
            "--take",
            "--detail",
            "--json",
            "--fail-on-revert",
            "--apply",
        ],
        text: r#"  adopt <export.xml>          take a designer's or backend collaborator's export; plan unless --apply
      --base <file|handoff|git rev>  collaborator's starting point; --only ui|backend
      --take theirs:<name>,ours:<name>  resolve conflicts; --entity <name>; --detail; --json; --fail-on-revert"#,
    },
    Command {
        path: "handoff",
        group: 1,
        operands: true,
        flags: &["--name", "--apply"],
        text: r#"  handoff list|record <file.xml>... --name <name> [--apply]
                              list recorded collaborator bases, or plan/copy one under .twaco/handoffs"#,
    },
    Command {
        path: "rename entity",
        group: 1,
        operands: true,
        flags: &[
            "--apply",
            "--text",
            "--detail",
            "--json",
            "--skip-checks",
            "--sql",
            "--sql-dir",
            "--no-sql",
        ],
        text: r#"  rename entity <old> <new> [--apply] [--text] [--detail] [--json] [--skip-checks]
                              rename exactly one entity; plan unless --apply
      --sql | --sql-dir <dir> | --no-sql   the database half, as for rename field"#,
    },
    Command {
        path: "rename prefix",
        group: 1,
        operands: true,
        flags: &[
            "--apply",
            "--text",
            "--detail",
            "--json",
            "--skip-checks",
            "--sql",
            "--sql-dir",
            "--no-sql",
        ],
        text: r#"  rename prefix <old> <new> [--apply] [--text] [--detail] [--json] [--skip-checks]
                              rename a project/building-block prefix; --text includes other files
      --sql | --sql-dir <dir> | --no-sql   the database half, as for rename field"#,
    },
    Command {
        path: "rename field",
        group: 1,
        operands: true,
        flags: &[
            "--apply",
            "--text",
            "--detail",
            "--json",
            "--skip-checks",
            "--sql",
            "--sql-dir",
            "--no-sql",
        ],
        text: r#"  rename field <datashape> <old> <new> [--apply] [--detail] [--json] [--skip-checks]
                              rename a DataShape field and its configuration tables
      --text                  refused: a field rename has no text pass
      --sql | --sql-dir <dir> | --no-sql
                              entity, prefix and field: a rename that touches DBConnection tables is
                              refused until you choose: --sql writes the migration script (default
                              folder sql/, run it before the import), --no-sql says the tables are unused"#,
    },
    Command {
        path: "rename service",
        group: 1,
        operands: true,
        flags: &["--apply", "--text", "--detail", "--json", "--skip-checks"],
        text: r#"  rename service <entity> <old> <new> [--apply] [--detail] [--json] [--skip-checks]
                              rename a declared service, its overrides and repository callers
      --text                  refused: a service rename has no outside-text pass"#,
    },
    Command {
        path: "rename param",
        group: 1,
        operands: true,
        flags: &["--apply", "--text", "--detail", "--json", "--skip-checks"],
        text: r#"  rename param <entity> <service> <old> <new> [--apply] [--detail] [--json] [--skip-checks]
                              rename one declared service input and repository callers
      --text                  refused: a parameter rename has no outside-text pass"#,
    },
    Command {
        path: "rename table",
        group: 1,
        operands: true,
        flags: &["--apply", "--text", "--detail", "--json", "--skip-checks"],
        text: r#"  rename table <entity> <old> <new> [--apply] [--detail] [--json] [--skip-checks]
                              rename a declared configuration table and inherited instances
      --text                  refused: a table rename has no outside-text pass"#,
    },
    Command {
        path: "rename property",
        group: 1,
        operands: true,
        flags: &["--apply", "--text", "--detail", "--json", "--skip-checks"],
        text: r#"  rename property <entity> <old> <new> [--apply] [--detail] [--json] [--skip-checks]
                              rename a property of a Thing, template or shape, its overrides and readers
      --text                  refused: a property rename has no text pass"#,
    },
    Command {
        path: "move service",
        group: 1,
        operands: true,
        flags: &["--as", "--leave-delegate", "--apply", "--detail", "--json"],
        text: r#"  move service <from> <to> <name> [--as <new>] [--leave-delegate] [--apply] [--detail] [--json]
                              lift a service out of one Thing, template or shape and put it on another
      --as <new>              give it a new name on the target
      --leave-delegate        keep the service on the source, calling the moved one (a Thing target)"#,
    },
    Command {
        path: "move property",
        group: 1,
        operands: true,
        flags: &["--as", "--apply", "--detail", "--json"],
        text: r#"  move property <from> <to> <name> [--as <new>] [--apply] [--detail] [--json]
                              the same for a property definition"#,
    },
    Command {
        path: "copy service",
        group: 1,
        operands: true,
        flags: &["--as", "--apply", "--detail", "--json"],
        text: r#"  copy service <from> <to> <name> [--as <new>] [--apply] [--detail] [--json]
                              as move, and the source keeps its service"#,
    },
    Command {
        path: "copy property",
        group: 1,
        operands: true,
        flags: &["--as", "--apply", "--detail", "--json"],
        text: r#"  copy property <from> <to> <name> [--as <new>] [--apply] [--detail] [--json]"#,
    },
    Command {
        path: "retemplate",
        group: 1,
        operands: true,
        flags: &[
            "--to",
            "--add-shapes",
            "--remove-shapes",
            "--accept-loss",
            "--apply",
            "--detail",
            "--json",
        ],
        text: r#"  retemplate <entity> [--to <template>] [--add-shapes a,b] [--remove-shapes a,b] [--accept-loss] [--apply] [--detail] [--json]
                              change a Thing's template (or a template's base) or its implemented shapes;
                              the plan lists what it and everything inheriting it gains and loses
      --to <template>         the new thingTemplate / baseThingTemplate
      --add-shapes a,b        implement these shapes too
      --remove-shapes a,b     stop implementing these shapes
      --accept-loss           go ahead although stored values or references would lose their definition"#,
    },
    Command {
        path: "new building-block",
        group: 1,
        operands: true,
        flags: &[
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
        text: r#"  new building-block <name> [--type standard|abstract|implementation] [--display-name <text>] [--description <text>]
      [--parent <block>] [--model-logic] [--no-management-shape] [--root <dir>] [--base-extension PTC.Base:<version>]
      [--apply] [--json]
                              create a building block: its project, entry point, manager, groups and
                              organization as files, and its project in twaco.toml; plan unless --apply
      --type                  standard (own manager, default), abstract (no manager Thing) or implementation
      --parent <block>        the abstract block an implementation implements
      --model-logic           also a ModelLogic_TS shape
      --no-management-shape   an implementation without its own Management_TS
      --root <dir>            the project's folder (default: the block's name)
      --base-extension        the PTC.Base extension version to depend on (default: what another project declares)"#,
    },
    Command {
        path: "entity status",
        group: 2,
        operands: true,
        flags: &["--all", "--project", "--profile", "--detail", "--record"],
        text: r#"  entity status [<entity>|--all] [--detail] [--record]
      --record                 record matching working/server hashes as baseline"#,
    },
    Command {
        path: "entity get",
        group: 2,
        operands: true,
        flags: &["--out", "--profile"],
        text: r#"  entity get <entity>         fetch raw server XML to stdout or --out <path>
                              Collection/Name: any server entity; a bare name: the repository's
      [--profile <name>]      server profile (default: default)"#,
    },
    Command {
        path: "entity push",
        group: 2,
        operands: true,
        flags: &["--apply", "--force", "--no-backup", "--profile"],
        text: r#"  entity push <entity>        import one entity, refusing if the server changed
      --apply                 actually push (without it, report what would happen)
      --force                 push over a server-side change; the server's copy is saved first
      --no-backup             with --force, do not save the server's copy first"#,
    },
    Command {
        path: "entity delete",
        group: 2,
        operands: true,
        flags: &[
            "--renamed",
            "--force",
            "--allow-repository-defined",
            "--allow-outside-dependents",
            "--allow-file-repository-data-loss",
            "--apply",
            "--no-backup",
            "--profile",
            "--json",
        ],
        text: r#"  entity delete <entity>...   plan guarded server deletion; Collection/Name or a bare server name
      --renamed               also delete undeleted old entity/prefix names from .twaco/renames.json
      --allow-repository-defined  accept deletion of an entity the repository still defines
      --allow-outside-dependents  accept structural dependents outside this delete set
      --allow-file-repository-data-loss  accept deletion of a FileRepository Thing and its files
      --force                 deprecated: means the first two acknowledgements, never FileRepository data loss
      --apply                 delete, confirm each entity is absent, and mark ledger entries
      --no-backup             do not save the server's copies under .twaco/backups first
      --json                  entities include refusal messages and parallel refusal_codes when refused"#,
    },
    Command {
        path: "entity carry",
        group: 2,
        operands: true,
        flags: &["--renamed", "--apply", "--detail", "--profile", "--json"],
        text: r#"  entity carry <old> <new>... copy run-time, design-time and visibility permissions of renamed entities
                              (pairs written Collection/Old Collection/New); principals follow the ledger
      --renamed               also every entity the rename ledger has not yet carried or deleted
      --apply                 write the differing permissions, read each back, mark the ledger
      --detail                also ask the platform for its own difference count
      --json                  {plan|applied, entities:[collection, old, new, status, kinds, error]}"#,
    },
    Command {
        path: "entity restore",
        group: 2,
        operands: true,
        flags: &["--apply", "--profile", "--json"],
        text: r#"  entity restore [<set> [<entity>...]]  list backup sets, or plan importing one back
      --apply                 import the set's entities, confirming each on the server
      --json                  {sets|plan|applied, ...}"#,
    },
    Command {
        path: "permissions init",
        group: 2,
        operands: false,
        flags: &["--project", "--from-helper", "--apply", "--json"],
        text: r#"  permissions init [--project <name>] [--from-helper] [--apply] [--json]
                              draft a permissions.toml for each project without one, from what it
                              grants today, so that `permissions apply` then changes nothing but
                              what its notes name; roles from the permission helper when there is
                              one; prints unless --apply
      --from-helper           take the grants from the helper's tables, not the entity XML (a
                              matrix edited in the helper's mashup)"#,
    },
    Command {
        path: "permissions audit",
        group: 2,
        operands: false,
        flags: &["--project", "--server", "--profile", "--detail", "--json"],
        text: r#"  permissions audit [--project <name>] [--server] [--detail] [--json]
                              each project's permissions.toml against its entity XML, offline: blocks
                              the policy would change, services a strict entity leaves unclassified,
                              principals the server refuses or no entity defines, rules that match
                              nothing; exit 1 on any error
      --server                also the server (read-only): each entity's permissions, the helper's
                              tables, the policy's platform grants and memberships, each role's unit
      --detail                every grant behind a finding"#,
    },
    Command {
        path: "permissions apply",
        group: 2,
        operands: false,
        flags: &["--project", "--apply", "--detail", "--json"],
        text: r#"  permissions apply [--project <name>] [--apply] [--detail] [--json]
                              write each project's permissions.toml into its entity XML: the run-time
                              block of each Thing, the instance run-time block of each shape and
                              template, and the role principals of each visibility block; only blocks
                              that differ change; plan unless --apply; refused while a strict
                              entity has an unclassified service
      --detail                every grant each file gains or loses"#,
    },
    Command {
        path: "permissions diff",
        group: 2,
        operands: true,
        flags: &["--all", "--project", "--profile", "--json"],
        text: r#"  permissions diff <entity>...|--all [--json]
                              each entity's run-time, design-time and visibility permissions (and a
                              shape's or template's instance permissions) against the server's:
                              grants only the server has (an import never removes one), grants only
                              the repository has, and allow/deny that differs (an import keeps the
                              server's); exit 1 when any differs"#,
    },
    Command {
        path: "permissions push",
        group: 2,
        operands: true,
        flags: &[
            "--all",
            "--platform",
            "--project",
            "--apply",
            "--profile",
            "--json",
        ],
        text: r#"  permissions push <entity>...|--all|--platform [--apply] [--json]
                              make the server's permission sets exactly the repository's, set by set,
                              reading each back; plan unless --apply; records the baseline of pushed
                              entities that then match the server
      --platform              instead, add the policies' [[platform]] grants and memberships the
                              server lacks (what DeployComponent does); never removes anything"#,
    },
    Command {
        path: "datatable copy",
        group: 2,
        operands: true,
        flags: &[
            "--map",
            "--drop-unmapped",
            "--append",
            "--max-rows",
            "--apply",
            "--profile",
            "--json",
        ],
        text: r#"  datatable copy <old> <new> [--map a=b,c=d] [--drop-unmapped] [--append] [--max-rows <n>] [--apply] [--json]
                              copy a DataTable's rows into the one that replaced it; plan unless --apply
      --map a=b,...           source field -> target field (same names and the rename ledger match otherwise)
      --drop-unmapped         leave behind source fields that have no target field
      --append                allow a target that already has rows"#,
    },
    Command {
        path: "bundle",
        group: 2,
        operands: false,
        flags: &["--backend-only", "--check", "--handoff", "--apply"],
        text: r#"  bundle [--backend-only] [--handoff <name>] [--apply]  one importable document
      --check                 report whether the bundle is current, write nothing"#,
    },
    Command {
        path: "deploy",
        group: 2,
        operands: false,
        flags: &[
            "--apply",
            "--force",
            "--backend-only",
            "--only",
            "--only-projects",
            "--skip-checks",
            "--no-backup",
            "--profile",
        ],
        text: r#"  deploy [--apply] [--force]  check, bundle, live-parse, conflict-check, import
      --backend-only          exclude configured UI collections
      --only <entity>         include only this entity (repeatable)
      --only-projects <a,b>   include only these projects
      --skip-checks           skip offline gates; live parse still runs
      --no-backup             with --force, do not save the server's copies first"#,
    },
    Command {
        path: "config-table",
        group: 2,
        operands: true,
        flags: &[
            "--backup",
            "--restore",
            "--apply",
            "--diff",
            "--detail",
            "--profile",
        ],
        text: r#"  config-table <thing> <table> one Thing's configuration table on the server
      --backup <file>         save it (never overwrites a file)
      --restore <file>        put a backup back; a plan unless --apply
      --diff                  compare with the entity XML in the repository
      --detail                every row, not only the first"#,
    },
    Command {
        path: "db run",
        group: 2,
        operands: true,
        flags: &[
            "--thing",
            "--no-transaction",
            "--timeout",
            "--apply",
            "--profile",
            "--json",
        ],
        text: r#"  db run <file.sql> [--thing <name>] [--no-transaction] [--timeout <seconds>] [--apply] [--json]
                              run one atomic SQLCommand; plan unless --apply"#,
    },
    Command {
        path: "db query",
        group: 2,
        operands: true,
        flags: &[
            "-q",
            "--thing",
            "--max-rows",
            "--timeout",
            "--profile",
            "--detail",
            "--json",
        ],
        text: r#"  db query <file.sql>|-q <sql> [--thing <name>] [--max-rows <n>] [--timeout <seconds>] [--detail] [--json]
                              run a read-only SQLQuery; summary shows columns and first 20 rows"#,
    },
    Command {
        path: "db clean",
        group: 2,
        operands: false,
        flags: &["--apply", "--profile", "--json"],
        text: r#"  db clean [--apply] [--json] delete temporary ZZ.Twaco.Sql.* Things an interrupted db run left; plan unless --apply"#,
    },
    Command {
        path: "call",
        group: 3,
        operands: true,
        flags: &["--timeout", "--detail", "--profile", "--with-logs"],
        text: r#"  call <target> <service> [<json>] [--timeout <seconds>] [--detail]
                              target: Collection/Name, or an entity by name or last segment
      --with-logs             then what it wrote to ScriptLog and ApplicationLog (waits up to 3 s)"#,
    },
    Command {
        path: "logs",
        group: 3,
        operands: true,
        flags: &[
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
        text: r#"  logs <log>                  read a server log: ApplicationLog, ScriptLog, CommunicationLog,
                              ConfigurationLog, SecurityLog (newest first; times are local)
      --since <n><s|m|h|d>    how far back (default 1h); or --from <time> --to <time>
      --level <L>             L and above: TRACE, DEBUG, INFO, WARN, ERROR
      --grep <text>           entries whose message contains the text
      --regex <re>            a Java regex that must match the whole message
      --user/--thread/--origin <v>  exact matches
      --limit <n>             at most n entries (default 100); --oldest-first; --json
  logs level <log> [<LEVEL>]  read a log's level and its subloggers, or plan setting it
      --sublogger <name>      a class or package within the log
      --reset                 a sublogger (or, without --sublogger, all) back to the log's level
      --apply                 make the change; it is the whole server's, and the undo is printed"#,
    },
    Command {
        path: "settings",
        group: 3,
        operands: true,
        flags: &["--search", "--json", "--profile"],
        text: r#"  settings                    the server's subsystems: running, and how many settings tables
  settings <Subsystem> [<Table>]  every setting of one, with its value and description
  settings --search <text>    settings whose name or description contains the text
                              (read-only; PASSWORD values are never shown); --json"#,
    },
    Command {
        path: "repo",
        group: 4,
        operands: true,
        flags: &[
            "--recursive",
            "--out",
            "--json",
            "--profile",
            "--force",
            "--overwrite",
            "--apply",
        ],
        text: r#"  repo list                   the server's file repositories
  repo ls <repo> [<path>]     a folder's folders and files; --recursive for all below it; --json
  repo get <repo> <path>      a file to stdout, or --out <file> (never overwritten without --force)
  repo status <repo>          filerepository/<repo>/ against the server: same, differs,
                              local-only, remote-only (sizes, then SHA-256); --json
  repo put <repo> <file> <path>  upload a file; --overwrite to replace one
  repo mkdir <repo> <path>    create a folder
  repo rm <repo> <path>       delete a file, or an empty folder; --recursive for one with content
  repo mv <repo> <from> <to>  move a file; --overwrite to replace one
  repo push <repo>            upload what filerepository/<repo>/ has that the server lacks
  repo pull <repo>            download what the server has into filerepository/<repo>/
                              (neither deletes; a file differing on both sides needs --overwrite)
      --apply                 make the change: put, mkdir, rm and mv are plans without it"#,
    },
    Command {
        path: "localization",
        group: 4,
        operands: true,
        flags: &[
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
        text: r#"  localization status         the solution's localization tables (localization/) against the
                              server: same, differs, local only, server only; problems; --json; --detail
  localization pull           write the server's tokens of this solution into the table files
  localization push           import the table files that differ, Default first, and read back
                              (--prune also deletes server tokens no file has, on pull removes
                              rows the server lacks)
  localization new <table>    a table file for a language, seeded with the Default tokens;
                              --language-common, --language-native, --description, --project
  localization set <token> --value <text>  add or change a token; --table (default Default),
                              --usage (new: label), --context, --project
  localization remove <token> remove a token from every table, or from --table
      --table <name>          one table only; --apply makes the change (all but status plan)"#,
    },
    Command {
        path: "ext",
        group: 4,
        operands: true,
        flags: &["--apply", "--json", "--profile"],
        text: r#"  ext list [--json]           the server's extension packages
  ext show <package>          one package: its extensions, and which are in use
  ext import <zip>            validate a package on the server; --apply installs it
  ext remove <package>        plan removing a package (refused while in use); --apply removes it"#,
    },
    Command {
        path: "search",
        group: 4,
        operands: true,
        flags: &["--type", "--project", "--limit", "--json", "--profile"],
        text: r#"  search [<text>] [--type <T>[,<T>]] [--project P] [--limit <n>] [--json]
                              the server's entity search (Composer's Spotlight): text
                              in a name or description, or a pattern with *"#,
    },
    Command {
        path: "export",
        group: 4,
        operands: true,
        flags: &[
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
        text: r#"  export entity <Coll/Name> --out <file>   one entity's XML, from the server's Exporter
  export collection <Coll> [--project P] --out <file>   a collection, or one project's part of it
  export project <P> --out <file>  everything of a project, as one XML
      --force                 replace the --out file if it exists
  export source-control --repository R --path <p> [--project P] [--collection C] [--tags t]
      [--zip <name>] [--with-dependents]  the source-control layout into a repository; --apply sends it"#,
    },
    Command {
        path: "import",
        group: 4,
        operands: true,
        flags: &[
            "--apply",
            "--overwrite-properties",
            "--overwrite-tables",
            "--repository",
            "--path",
            "--profile",
            "--detail",
        ],
        text: r#"  import <file.xml|.zip>      import an export into the server: a plan of what it adds and
                              replaces unless --apply
  import source-control --repository R --path <p>  a source-control tree from a repository;
                              the plan is the server's diff; --apply imports, then diffs again
      --detail                every entity of the plan, not only the counts
      --overwrite-properties --overwrite-tables  replace the server's property values and
                              configuration table rows (kept by default, as in Composer)"#,
    },
    Command {
        path: "package",
        group: 5,
        operands: true,
        flags: &[
            "--project",
            "--backend-only",
            "--frontend-only",
            "--editable",
            "--out",
            "--force",
        ],
        text: r#"  package bundle [--project P] [--backend-only|--frontend-only] --out <file>
                              the repository as one importable XML, offline
  package source-control [--project P] --out <file.zip>  the <Project>/<Collection>/<Name>.xml layout
  package extension [--project P] [--editable] --out <file.zip>  an extension package: one project's,
                              or the solution's as a zip of its projects' ([package] in twaco.toml)
      --force                 replace the --out file if it exists"#,
    },
    Command {
        path: "guide",
        group: 6,
        operands: true,
        flags: &["--section", "--search", "--limit", "--json"],
        text: r#"  guide                       the knowledge topics: twaco's workflow, the platform's verified quirks,
                              the service-code reference, and the solution's own markdown
  guide <topic> [--section <heading>]  read one (a long one gives its outline)
  guide --search <words>      the sections that best match; --limit <n>; --json"#,
    },
    Command {
        path: "help",
        group: 6,
        operands: true,
        flags: &[
            "--version",
            "--limit",
            "--section",
            "--refresh",
            "--json",
            "--profile",
        ],
        text: r#"  help search <words>         search the ThingWorx Platform help (the server's version)
  help page <page>            read a help page as Markdown; --section <heading>
      --version <10.1>        another release; --limit <n>; --refresh; --json
      --profile <name>        the server whose version to read (default: default)"#,
    },
    Command {
        path: "javadoc",
        group: 6,
        operands: true,
        flags: &["--member", "--limit", "--refresh", "--json"],
        text: r#"  javadoc search <name>       search ThingWorx Platform Java API classes and members
      --limit <n>; --refresh; --json
  javadoc class <Name|pkg.Name>  read a class as Markdown; --member <name>; --refresh; --json"#,
    },
];

/// The sections of the listing and of COMMANDS.md: a title and one line about them.
pub(crate) const GROUPS: &[(&str, &str)] = &[
    ("Set up", "Describe a solution, check the environment, serve agents, update twaco."),
    ("Work on the repository", "Offline: sidecars, gates, types, the service catalog, what a change reaches, a designer's export, renames."),
    ("Deploy and compare", "Against a server: what differs, what would be imported, and doing it."),
    ("Run and observe", "Call services and read what the server says."),
    ("Server content", "File repositories, extension packages, Composer-style exports and imports."),
    ("Release", "Package the repository, offline."),
    ("Knowledge", "Workflow, platform quirks, project documents, the help center and the Java API."),
];

/// The end of the listing: the flags described once, and the exit codes.
pub(crate) const FOOTER: &str = r#"  --project <name>            narrow to one project of the solution
  --profile <name>            the server profile (default: default)
  --log <filter>              diagnostic logs on stderr, for any command (or TWACO_LOG);
                              a level such as debug, or a directive such as twaco::core::server=trace
  --log-file <path>           append diagnostic logs to a file instead (or TWACO_LOG_FILE)
  --version                   twaco's version and the commit it was built from
  <command> --help            one command, and its flags

exit: 0 done, 1 a --check found work, 2 failed"#;
