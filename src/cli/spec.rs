//! Every command, once. [`USAGE`] says what each is: its path, its group and the text `twaco`
//! with no arguments, `--help` and `documentation/COMMANDS.md` show. What each takes is its
//! struct, beside its handler, as a variant of [`Command`]; clap parses against the tree [`tree`]
//! builds from them (unknown flags and operands are refused, with a suggestion).
//!
//! How each kind of argument behaves on the command line (a switch said twice, a value that
//! starts with a dash, a negative operand) is decided once, in [`decorate`], so a struct declares
//! only what its flags are called and what their values are named.

use super::command::Command;

/// A command as people read it.
#[derive(Debug)]
pub(crate) struct Usage {
    /// One word, or two for a family (`entity push`).
    pub(crate) path: &'static str,
    /// Its section in [`GROUPS`].
    pub(crate) group: usize,
    /// What `twaco` with no arguments shows for it: synopsis lines at two spaces, flag lines at
    /// six, descriptions from column 31. A test holds it to every flag the command takes.
    pub(crate) text: &'static str,
}

/// The command a path names.
pub(crate) fn command(path: &str) -> Option<&'static Usage> {
    USAGE.iter().find(|command| command.path == path)
}

/// The clap id of a flag: its name without dashes.
fn id(name: &str) -> &str {
    name.trim_start_matches('-')
}

/// Flags every command accepts without listing them, and their value names: diagnostic logs. See
/// `core::diagnostics`.
pub(crate) const GLOBAL_FLAGS: &[(&str, &str)] = &[("--log", "filter"), ("--log-file", "path")];

/// The diagnostic options a parse matched: `--log` and `--log-file`.
pub(crate) fn log_options(matches: &clap::ArgMatches) -> (Option<String>, Option<String>) {
    let get = |name: &str| matches.get_one::<String>(id(name)).cloned();
    (get("--log"), get("--log-file"))
}

/// The first synopsis of a command, as clap shows it in an error.
fn synopsis(command: &Usage) -> String {
    let first = command.text.lines().next().unwrap_or_default().trim();
    let synopsis = first.split("  ").next().unwrap_or(first).trim();
    format!("twaco {synopsis}")
}

/// The diagnostic flags, [`GLOBAL_FLAGS`]; [`decorate`] adds how they behave.
fn log_arguments() -> impl Iterator<Item = clap::Arg> {
    GLOBAL_FLAGS.iter().map(|(name, value)| {
        clap::Arg::new(id(name))
            .long(id(name))
            .value_name(*value)
            .action(clap::ArgAction::Set)
    })
}

/// The whole command line: every command of [`Command`], with families as nested subcommands, in
/// the order [`USAGE`] lists them.
pub(crate) fn tree() -> clap::Command {
    use clap::Subcommand as _;
    let derived = Command::augment_subcommands(clap::Command::new("derived"));
    let mut root = clap::Command::new("twaco");
    // A family (`entity`, `rename`...) is a command of its own whose commands are the words
    // after it; it is listed where its first command is.
    let mut added: Vec<&str> = Vec::new();
    for command in USAGE {
        let word = command.path.split(' ').next().unwrap_or(command.path);
        if added.contains(&word) {
            continue;
        }
        added.push(word);
        let built = derived
            .find_subcommand(word)
            .unwrap_or_else(|| panic!("{} is in USAGE but not in Command", command.path));
        root = root.subcommand(built.clone());
    }
    decorate(root)
}

/// The grammar's policy, applied once to the whole tree: the root's and each family's listing and
/// settings, and on every command its usage line and help, the diagnostic flags, and how each
/// kind of argument behaves.
pub(crate) fn decorate(root: clap::Command) -> clap::Command {
    decorate_node(root, "")
}

/// One node of the tree, `path` the words before and including its own (empty for the root).
fn decorate_node(node: clap::Command, path: &str) -> clap::Command {
    if path.is_empty() {
        return node
            // Built once per run; clap keeps the version as a static string.
            .version(&*Box::leak(twaco::version().into_boxed_str()))
            .override_usage("twaco <command>")
            .override_help(listing())
            .disable_help_subcommand(true)
            .subcommand_required(true)
            .arg_required_else_help(true)
            .mut_subcommands(|sub| {
                let path = sub.get_name().to_string();
                decorate_node(sub, &path)
            });
    }
    if let Some(command) = command(path) {
        return leaf(node, command);
    }
    // What shell completions say a family is: its commands. Not the doc comment clap derive
    // would take, which is written for the code.
    let members: Vec<&str> = USAGE
        .iter()
        .filter_map(|c| c.path.strip_prefix(path)?.strip_prefix(' '))
        .collect();
    node.override_usage(format!("twaco {path} <command>"))
        .override_help(family_listing(path))
        .about(members.join(", "))
        .long_about(None::<&str>)
        .subcommand_required(true)
        .arg_required_else_help(true)
        .disable_help_subcommand(true)
        .disable_version_flag(true)
        .mut_subcommands(|sub| {
            let path = format!("{path} {}", sub.get_name());
            decorate_node(sub, &path)
        })
}

/// A command: its usage line and help from [`USAGE`], the diagnostic flags, and the policy for
/// every argument. `--help` shows the text as written; the one-line descriptions of the command
/// and of each flag are taken from it too, for shell completions.
fn leaf(node: clap::Command, command: &'static Usage) -> clap::Command {
    let takes = |long: &str| node.get_arguments().any(|arg| arg.get_long() == Some(long));
    let help = help_of(command, &|name| takes(id(name)));
    // clap derive takes a struct's doc comment for its description; completions show the help
    // text's instead, or none.
    let node = match summary(command.text) {
        Some(about) => node.about(about),
        None => node.about(None::<&str>),
    }
    .long_about(None::<&str>);
    node.override_usage(synopsis(command))
        .override_help(help)
        .disable_version_flag(true)
        .args(log_arguments())
        .mut_args(|arg| {
            let name = arg
                .get_long()
                .map(|long| format!("--{long}"))
                .or_else(|| arg.get_short().map(|short| format!("-{short}")));
            let help = name.and_then(|name| {
                [command.text, SHARED, LOGGING]
                    .into_iter()
                    .find_map(|text| described(text, &name))
            });
            let arg = behaviour(arg).long_help(None::<&str>);
            match help {
                Some(help) => arg.help(help),
                None => arg.help(None::<&str>),
            }
        })
}

/// Where a description starts in the help text: column 31.
const DESCRIPTION_COLUMN: &str = "                              ";

/// A synopsis or flag entry split into what it names and what it says it does: they are parted by
/// the first run of two spaces.
fn entry(line: &str) -> (&str, &str) {
    let line = line.trim_start();
    match line.find("  ") {
        Some(at) => (&line[..at], line[at..].trim()),
        None => (line, ""),
    }
}

/// A description and the lines that continue it at [`DESCRIPTION_COLUMN`], as one line.
fn joined<'a>(first: &'a str, rest: impl Iterator<Item = &'a str>) -> Option<String> {
    let words: Vec<&str> = std::iter::once(first)
        .chain(
            rest.take_while(|line| line.starts_with(DESCRIPTION_COLUMN) && !line.trim().is_empty()),
        )
        .flat_map(str::split_whitespace)
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// What a command's text says the command does: its first entry's description.
fn summary(text: &str) -> Option<String> {
    let mut lines = text.lines();
    let (_, first) = entry(lines.next()?);
    joined(first, lines)
}

/// What a help text says a flag does, when the flag has a line of its own
/// (`      --when <address>        the event...`); a flag named only in passing has none.
fn described(text: &str, flag: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    lines.iter().enumerate().find_map(|(at, line)| {
        let (head, first) = entry(line);
        let own = head.starts_with('-')
            && head.match_indices(flag).any(|(start, _)| {
                let before = head[..start].chars().next_back();
                let after = head[start + flag.len()..].chars().next();
                let word =
                    |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '-');
                !word(before) && !word(after)
            });
        own.then(|| joined(first, lines[at + 1..].iter().copied()))
            .flatten()
            // `another release; --limit <n>; --json` describes this flag up to the next one.
            .map(|text| match text.split_once("; -") {
                Some((own, _)) => own.to_string(),
                None => text,
            })
    })
}

/// How an argument behaves, decided by what it takes.
fn behaviour(arg: clap::Arg) -> clap::Arg {
    if arg.is_positional() && arg.get_id() != "operands" {
        // A sub-action (`users list`) or a shell: one word of those it takes.
        return arg;
    }
    if arg.is_positional() {
        // A negative number is an operand (a JSON value for `call`); any other word that
        // starts with a dash is a flag, so a mistyped one is refused, not taken as a name.
        // A file whose name starts with a dash goes after `--`. None is as good as several:
        // the command says what it needs.
        return arg.num_args(0..).allow_negative_numbers(true);
    }
    let optional = arg
        .get_num_args()
        .is_some_and(|range| range.min_values() == 0);
    match arg.get_action() {
        // Saying a switch twice says it once, as it always did. A switch takes no value, so it
        // has no value name (clap derive gives every field one).
        clap::ArgAction::SetTrue => {
            let own = arg.get_id().clone();
            arg.overrides_with(own)
                .value_names(std::iter::empty::<&str>())
        }
        clap::ArgAction::Set if optional => arg
            .default_missing_value("")
            // A value may be negative, but a flag after the bare form is the next flag,
            // not this one's value (`--default --label Main`).
            .allow_negative_numbers(true),
        // A list of values ends at the next flag (`--from-server A B --apply`).
        clap::ArgAction::Append
            if arg
                .get_num_args()
                .is_some_and(|range| range.max_values() > 1) =>
        {
            arg.allow_negative_numbers(true)
        }
        // A value may start with a dash, as a log filter can.
        clap::ArgAction::Set | clap::ArgAction::Append => arg.allow_hyphen_values(true),
        _ => arg,
    }
}

/// The command a parse matched, by its [`USAGE`] path, and the matches of that command.
pub(crate) fn matched(matches: &clap::ArgMatches) -> (&'static str, &clap::ArgMatches) {
    let (first, sub) = matches.subcommand().expect("a subcommand is required");
    if let Some(command) = command(first) {
        return (command.path, sub);
    }
    let (second, leaf) = sub.subcommand().expect("a family requires a command");
    let command = USAGE
        .iter()
        .find(|c| c.path.split_once(' ') == Some((first, second)))
        .expect("clap matched only commands in USAGE");
    (command.path, leaf)
}

/// Flags every command describes once, at the end of the listing.
const SHARED: &str = "  --project <name>            narrow to one project of the solution\n  --profile <name>            the server profile (default: default)";

/// The diagnostic flags: any command takes them, so every command's help ends with them.
const LOGGING: &str = "  --log <filter>              write diagnostic logs to stderr (or TWACO_LOG)\n  --log-file <path>           append diagnostic logs to a file (or TWACO_LOG_FILE)";

/// `twaco <command> --help`: its block, then the shared flags it takes (`takes` answers for a
/// flag name).
fn help_of(command: &Usage, takes: &dyn Fn(&str) -> bool) -> String {
    let shared: Vec<&str> = SHARED
        .lines()
        .filter(|line| takes(line.split_whitespace().next().unwrap_or_default()))
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
        for command in USAGE.iter().filter(|c| c.group == group) {
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
    for command in USAGE
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
        for command in USAGE.iter().filter(|c| c.group == group) {
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

/// Every command as people read it, in the order `twaco` lists them within a group.
pub(crate) const USAGE: &[Usage] = &[
    Usage {
        path: "init",
        group: 0,
        text: r#"  init [--write]              propose a twaco.toml from the repository's own entities; --write
                              also writes AGENTS.md and CLAUDE.md where absent, and adds the
                              lines twaco needs to .gitignore
  init --agents               write only those, where absent
  init --from-server <project>... [--profile <name>] [--apply] [--json]
                              start a repository in an empty folder, or add projects to this one,
                              from server projects; plans by default
      --profile <name>        the server profile (default: default)
      --apply                 write the planned repository and record matching baselines
      --json                  print the plan or result as JSON"#,
    },
    Usage {
        path: "doctor",
        group: 0,
        text: r#"  doctor [--profile <name>] [--json]  what resolved, what is reachable, what is missing
      --json                  print the report as JSON"#,
    },
    Usage {
        path: "projects",
        group: 0,
        text: r#"  projects [--json]           the solution's projects and their deploy order
      --json                  print the report as JSON"#,
    },
    Usage {
        path: "mcp",
        group: 0,
        text: r#"  mcp [--tools <groups>]      serve the tools over MCP on stdio (TWACO_ROOT: the solution)
      --tools <groups>        only these tool groups, comma-separated (TWACO_MCP_TOOLS): core,
                              entity, refactor, mashup, permissions, data, server, reference;
                              default all"#,
    },
    Usage {
        path: "update",
        group: 0,
        text: r#"  update [--apply]            compare with the latest release; --apply verifies its signature
                              and replaces this binary (TWACO_NO_UPDATE_CHECK=1: no daily notice)"#,
    },
    Usage {
        path: "completions",
        group: 0,
        text: r#"  completions <shell>         a completion script on stdout for bash, elvish, fish, powershell
                              or zsh: commands and flags with descriptions; actions in bash and zsh"#,
    },
    Usage {
        path: "extract",
        group: 1,
        text: r#"  extract <entity>|--all      entity XML -> sidecars"#,
    },
    Usage {
        path: "sync",
        group: 1,
        text: r#"  sync <entity>|--all         sidecars -> entity XML
      --check                 report what would change, write nothing
      --allow-add-remove      add and remove services and fields as the sidecars do
      --relayout              rewrite scripts in the configured CDATA layout"#,
    },
    Usage {
        path: "fmt",
        group: 1,
        text: r#"  fmt [--check]               format service scripts"#,
    },
    Usage {
        path: "check",
        group: 1,
        text: r#"  check [--detail]            every gate, one exit code
      --live                  also parse every script on the server (fails closed)"#,
    },
    Usage {
        path: "types",
        group: 1,
        text: r#"  types [--check [--json]|--platform] [--profile <name>]
                              generate declarations; --check runs TypeScript once
                              suppress one finding on the line above with
                              // @ts-ignore or // @ts-expect-error
                              [[check]] hook: command = ["twaco", "types", "--check", "--json"]"#,
    },
    Usage {
        path: "catalog",
        group: 1,
        text: r#"  catalog [<entity>|<Entity>.<Service>|<Service>] [--project P] [--search <text>] [--json]
                              offline services, signatures, origins and descriptions; an
                              Entity.Service, or a service only one entity declares, gives that one
      --body                  with an entity or service: each service's script too"#,
    },
    Usage {
        path: "contract diff",
        group: 1,
        text: r#"  contract diff --against <git rev> [--project P] [--detail] [--json]
                              compare public contracts with a git revision; offline and read-only
      --against <git rev>     required revision to compare; try origin/main
      --project <name>        limit both sides to this project
      --detail                every change and every caller in this repository
      --json                  the complete report as JSON; exit 1 when breaking"#,
    },
    Usage {
        path: "impact",
        group: 1,
        text: r#"  impact <entity> [--member <name>] [--min-confidence structural|resolved|review] [--depth <n>]
                              what changing an entity, or one service/property/field of it, reaches
      --detail                list every dependent and the chain of references to each
      --json                  the report as JSON (chains with --detail)
      --dot                   the dependents as a Graphviz graph"#,
    },
    Usage {
        path: "trace",
        group: 1,
        text: r#"  trace <Entity>.<Service> [--param <name>] [--json]
                              how each caller uses a service: what runs each mashup call, what sets
                              each parameter, where each result field goes, the arguments each script
                              passes and what it reads of the result, and what the service calls
      --param <name>          only that parameter's sources"#,
    },
    Usage {
        path: "unused",
        group: 1,
        text: r#"  unused [--min-confidence structural|resolved|review] [--collection <name>] [--fields]
                              entities no entry point reaches; advisory, deletes nothing
      --fields                result fields mashups bind, which no mashup binds or names
      --detail                list every one, with the entry points and each file
      --json                  the report as JSON"#,
    },
    Usage {
        path: "docs",
        group: 1,
        text: r#"  docs [--out <file>] [--force | --check]
                              the solution written down: projects and deploy order, inheritance,
                              services, DataShapes, and the references to review; Markdown
      --detail                every signature and field, and every review reference
      --json                  JSON instead of Markdown
      --out <file>            write it to one file (atomically) instead of printing; --force replaces
      --check                 with --out: write nothing, exit 1 when the file is missing or stale"#,
    },
    Usage {
        path: "adopt",
        group: 1,
        text: r#"  adopt <export.xml>          take a designer's or backend collaborator's export; plan unless --apply
      --base <file|handoff|git rev>  collaborator's starting point; --only ui|backend
      --take theirs:<name>,ours:<name>  resolve conflicts (theirs:ui takes every UI one)
      --entity <name>; --detail; --json; --fail-on-revert"#,
    },
    Usage {
        path: "handoff",
        group: 1,
        text: r#"  handoff list|record <file.xml>... --name <name> [--apply]
                              list recorded collaborator bases, or plan/copy one under .twaco/handoffs"#,
    },
    Usage {
        path: "rename entity",
        group: 1,
        text: r#"  rename entity <old> <new> [--apply] [--text] [--detail] [--json] [--skip-checks]
                              rename exactly one entity; plan unless --apply
      --sql | --sql-dir <dir> | --no-sql   the database half, as for rename field"#,
    },
    Usage {
        path: "rename prefix",
        group: 1,
        text: r#"  rename prefix <old> <new> [--apply] [--text] [--detail] [--json] [--skip-checks]
                              rename a project/building-block prefix; --text includes other files
      --sql | --sql-dir <dir> | --no-sql   the database half, as for rename field"#,
    },
    Usage {
        path: "rename field",
        group: 1,
        text: r#"  rename field <datashape> <old> <new> [--apply] [--detail] [--json] [--skip-checks]
                              rename a DataShape field and its configuration tables
      --text                  refused: a field rename has no text pass
      --sql | --sql-dir <dir> | --no-sql
                              entity, prefix and field: a rename that touches DBConnection tables is
                              refused until you choose: --sql writes the migration script (default
                              folder sql/, run it before the import), --no-sql says the tables are unused"#,
    },
    Usage {
        path: "rename service",
        group: 1,
        text: r#"  rename service <entity> <old> <new> [--apply] [--detail] [--json] [--skip-checks]
                              rename a declared service, its overrides and repository callers
      --text                  refused: a service rename has no outside-text pass"#,
    },
    Usage {
        path: "rename param",
        group: 1,
        text: r#"  rename param <entity> <service> <old> <new> [--apply] [--detail] [--json] [--skip-checks]
                              rename one declared service input and repository callers
      --text                  refused: a parameter rename has no outside-text pass"#,
    },
    Usage {
        path: "rename table",
        group: 1,
        text: r#"  rename table <entity> <old> <new> [--apply] [--detail] [--json] [--skip-checks]
                              rename a declared configuration table and inherited instances
      --text                  refused: a table rename has no outside-text pass"#,
    },
    Usage {
        path: "rename property",
        group: 1,
        text: r#"  rename property <entity> <old> <new> [--apply] [--detail] [--json] [--skip-checks]
                              rename a property of a Thing, template or shape, its overrides and readers
      --text                  refused: a property rename has no text pass"#,
    },
    Usage {
        path: "move service",
        group: 1,
        text: r#"  move service <from> <to> <name> [--as <new>] [--leave-delegate] [--apply] [--detail] [--json]
                              lift a service out of one Thing, template or shape and put it on another
      --as <new>              give it a new name on the target
      --leave-delegate        keep the service on the source, calling the moved one (a Thing target)"#,
    },
    Usage {
        path: "move property",
        group: 1,
        text: r#"  move property <from> <to> <name> [--as <new>] [--apply] [--detail] [--json]
                              the same for a property definition"#,
    },
    Usage {
        path: "copy service",
        group: 1,
        text: r#"  copy service <from> <to> <name> [--as <new>] [--apply] [--detail] [--json]
                              as move, and the source keeps its service"#,
    },
    Usage {
        path: "copy property",
        group: 1,
        text: r#"  copy property <from> <to> <name> [--as <new>] [--apply] [--detail] [--json]"#,
    },
    Usage {
        path: "retemplate",
        group: 1,
        text: r#"  retemplate <entity> [--to <template>] [--add-shapes a,b] [--remove-shapes a,b] [--accept-loss] [--apply] [--detail] [--json]
                              change a Thing's template (or a template's base) or its implemented shapes;
                              the plan lists what it and everything inheriting it gains and loses
      --to <template>         the new thingTemplate / baseThingTemplate
      --add-shapes a,b        implement these shapes too
      --remove-shapes a,b     stop implementing these shapes
      --accept-loss           go ahead although stored values or references would lose their definition"#,
    },
    Usage {
        path: "reproject",
        group: 1,
        text: r#"  reproject <entity>... --to <project> [--apply] [--json]
                              move entities to another project: their projectName, and their files
                              under that project's root; the plan names each reference the move puts
                              outside the deploy order, with the depends_on that allows it
      --to <project>          the project they move to"#,
    },
    Usage {
        path: "widgets",
        group: 1,
        text: r#"  widgets [<widget>] [--search <text>] [--json]
                              the embedded ThingWorx 10.2 widget catalogue: types, Composer labels,
                              aliases, containers and bindable properties, events and services"#,
    },
    Usage {
        path: "mashup validate",
        group: 1,
        text: r#"  mashup validate <mashup>|--all [--json]
                              what in a mashup ThingWorx would load but not honour, or the next edit would
                              trip over: structure, ids, bindings and events that name what is not there,
                              catalogue contracts, several property maps, contained-mashup drift;
                              exit 1 when it finds any"#,
    },
    Usage {
        path: "mashup describe",
        group: 1,
        text: r#"  mashup describe <mashup> [--json]
                              what a mashup holds and how it is wired, without its XML: the widget tree,
                              each service it calls with what sets each parameter (or that nothing does),
                              each binding and each event, in the addresses mashup bind and on take"#,
    },
    Usage {
        path: "mashup add",
        group: 1,
        text: r#"  mashup add <mashup> <widget> [--in <container> | --right-of <widget> | --left-of <widget> | --above <widget> | --below <widget>]
      [--label <text>] [--prop <name=value>]... [--input <name:TYPE>]... [--apply] [--json]
                              add a catalogue widget to a mashup; plan unless --apply; a function widget
                              (expression, validator, navigation...) goes beside the layout
      --in <container>        add inside a responsive container (center when omitted)
      --right-of/--left-of/--above/--below <widget>
                              place beside an existing widget
      --label <text>          set the widget's LabelText, Label, Text or Title property
      --prop <name=value>     set one simple catalogue default property, repeatable
      --input <name:TYPE>     an expression's or validator's input, repeatable"#,
    },
    Usage {
        path: "mashup add-contained",
        group: 1,
        text: r#"  mashup add-contained <parent> <child> [--in <container> | --right-of <widget> | --left-of <widget> | --above <widget> | --below <widget>]
      [--set <parameter=value>]... [--apply] [--json]
                              add a child mashup as a contained mashup; plan unless --apply
      --in <container>        add inside a responsive container (center when omitted)
      --right-of/--left-of/--above/--below <widget>
                              place beside an existing widget
      --set <parameter=value> set a visible input parameter on the child"#,
    },
    Usage {
        path: "mashup bind",
        group: 1,
        text: r#"  mashup bind <mashup> --from <address> --to <address> [--apply] [--json]
                              add one data binding to a mashup; plan unless --apply
      --from <address>        source address from mashup describe
      --to <address>          target address from mashup describe"#,
    },
    Usage {
        path: "mashup on",
        group: 1,
        text: r#"  mashup on <mashup> --event <address> --invoke <address> [--apply] [--json]
                              run a service or widget service when a mashup event fires; plan unless --apply
      --event <address>       event address from mashup describe
      --invoke <address>      data call or widget service to run"#,
    },
    Usage {
        path: "mashup param",
        group: 1,
        text: r#"  mashup param <add|set|remove> <mashup> <name> [--type <BASETYPE>] [--direction <IN|OUT|BOTH>]
      [--description <text>] [--default <value> | --no-default] [--data-shape <DataShape>] [--force] [--apply] [--json]
                              change a mashup parameter and every contained-mashup cache; plan unless --apply
      --type <BASETYPE>       required by add; ThingWorx base type
      --direction              IN, OUT or BOTH (BOTH by default for add)
      --default                a scalar default value
      --no-default             clear the default on set
      --data-shape             required for INFOTABLE
      --force                  remove bindings when removing a parameter"#,
    },
    Usage {
        path: "mashup recipe",
        group: 1,
        text: r#"  mashup recipe <recipe> <mashup> [<widget>] [recipe flags] [--apply] [--json]
                              write the wiring real mashups repeat, in one edit; plan unless --apply
      enable-when|show-when <widget> --if <condition> [--input name=<address>]...
                              an expression into the widget's Disabled or Visible
      text <label> --expr <expression> [--input name=<address>]...
                              a label's text computed from its inputs
      message --after <data call> (--say <text> | --from <address>) [--kind info|warning|error]
                              a status message when the call completes
      confirm <button> --then <handler> [--title <text>] [--say <text>]
                              a confirmation before the click runs the handler
      popup <widget>|--on <event> --mashup <name> [--window modal-popup|popup|replace|new]
          [--size <WxH>] [--title <text>] [--pass name=<address>]... [--reload <handler>]
                              open a mashup, pass it values, run the handler when it closes
      close-after --after <data call>
                              the popup closes itself when the call completes
      auto-refresh --then <data call> (--every <seconds> | --interval <address>)
                              run the call again every so often
      validate <widget>|--on <event> --if <condition> [--input name=<address>]... --then <handler>
          [--else-say <text>]   run the handler when the condition holds, else show the text
      chart <chart> --data <data call> --x <field> [--value <field>] [--kind <chart type>]
                              draw a service result: the category field (a pie: label and value)
      columns <grid> --column field[:Title[:width]]... [--data <data call>]
                              a grid's columns, rendered by each field's base type
      picker <dropdown|list> --data <data call> --x <display field> [--value <field>]
          [--selected <address>]   fill a dropdown or list from a service result"#,
    },
    Usage {
        path: "mashup event",
        group: 1,
        text: r#"  mashup event add <mashup> <name> --when <address> [--apply] [--json]
                              give a mashup an event its parents react to with mashup on --event
                              ui:<container>.<name>: a NUMBER parameter set to the time when the
                              event at --when fires; plan unless --apply
      --when <address>        the event in this mashup that fires it, as mashup on --event takes it"#,
    },
    Usage {
        path: "mashup templates",
        group: 1,
        text: r#"  mashup templates [<name>] [--json]
                              list the embedded responsive mashup templates and their slots"#,
    },
    Usage {
        path: "mashup new",
        group: 1,
        text: r#"  mashup new <name> [--project <name>] [--template <name>] [--primary-menu <address>]
      [--secondary-menu <address>] [--theme <StyleTheme>] [--master]
      [--visibility <principal>]... [--param <name:TYPE[:DIRECTION]>]... [--tab <Label=Mashup>]...
      [--apply] [--json]
                              create a ThingWorx 10.2 responsive mashup as Composer would: its entity file
                              and its content sidecars; plan unless --apply
      --template              layout template name or alias (default: blank); see mashup templates
      --layout                compatibility alias for --template
      --primary-menu          required INFOTABLE service result for a template's menu bar
      --secondary-menu        optional INFOTABLE service result for a template's menu bar
      --theme <StyleTheme>    a StyleTheme in the solution (default: PTC Convergence Theme)
      --master                a master mashup, with a page container filling the center
      --visibility            Organization/<name> or OrganizationalUnit/<organization>:<unit>, repeatable;
                              none unless given
      --tab                   with --template tabs: a tab's label and the mashup it shows, repeatable"#,
    },
    Usage {
        path: "new building-block",
        group: 1,
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
    Usage {
        path: "new project",
        group: 1,
        text: r#"  new project <name> [--description <text>] [--depends-on <P>]... [--apply] [--json]
                              create a ThingWorx Project and add it to twaco.toml when needed; plan unless --apply"#,
    },
    Usage {
        path: "new group",
        group: 1,
        text: r#"  new group <name> [--project <name>] [--description <text>] [--apply] [--json]
                              create a ThingWorx Group without members as a repository file; plan unless --apply"#,
    },
    Usage {
        path: "thing property",
        group: 1,
        text: r#"  thing property add <Entity> <name>:<TYPE> [--default <value>] [--persistent] [--logged]
      [--read-only] [--category <text>] [--units <text>] [--data-shape <DataShape>]
      [--description <text>] [--apply] [--json]
  thing property set <Entity> <name>[:<TYPE>] [the add flags] [--no-default] [--no-persistent]
      [--no-logged] [--no-read-only] [--force] [--apply] [--json]
  thing property remove <Entity> <name> [--force] [--apply] [--json]
                              add, change or remove a property of a Thing, ThingTemplate or
                              ThingShape; a persistent one also writes the stored value of every
                              Thing that has it; plan unless --apply
      --force                 remove or retype a property something still uses (they are listed)"#,
    },
    Usage {
        path: "thing service",
        group: 1,
        text: r#"  thing service add <Entity> <Name> [--param <name>:<TYPE>[:<DataShape>]]...
      [--returns <TYPE>[:<DataShape>]] [--description <text>] [--category <text>] [--private]
      [--then <code> | --script <file.js>] [--apply] [--json]
  thing service remove <Entity> <Name> [--force] [--apply] [--json]
                              add or remove an own script service and its managed sidecars; an
                              absent script is empty; plan unless --apply
      --param                 a parameter in declaration order
      --returns               result type, NOTHING by default
      --private               make the service private
      --force                 remove it even when callers still use it (they are listed)"#,
    },
    Usage {
        path: "thing subscription",
        group: 1,
        text: r#"  thing subscription add <Entity> <name> --event <Event> [--source <Thing>]
      [--source-property <property>] (--then <code> | --script <file.js>) [--apply] [--json]
  thing subscription remove <Entity> <name> [--apply] [--json]
                              add or remove a script subscription; no --source is the entity
                              itself; plan unless --apply"#,
    },
    Usage {
        path: "thing shape",
        group: 1,
        text: r#"  thing shape <add|remove> <Thing|ThingTemplate> <ThingShape> [--force] [--apply] [--json]
                              implement a ThingShape, or stop; refused when a member clashes, or
                              when something still uses a member the entity loses; plan unless --apply
      --force                 stop implementing it anyway (what uses its members is listed)"#,
    },
    Usage {
        path: "timer set",
        group: 1,
        text: r#"  timer set <Timer> [--every <duration>] [--run-as <user>] [--enabled|--disabled] [--apply]
      [--json]
                              change a Timer Thing's settings; plan unless --apply"#,
    },
    Usage {
        path: "scheduler set",
        group: 1,
        text: r#"  scheduler set <Scheduler> [--cron <quartz cron>] [--run-as <user>] [--enabled|--disabled]
      [--apply] [--json]
                              change a Scheduler Thing's settings; plan unless --apply"#,
    },
    Usage {
        path: "style set",
        group: 1,
        text: r#"  style set <StyleDefinition> [--background <color>] [--foreground <color>]
      [--secondary-background <color>] [--line-color <color>] [--line-style <style>]
      [--line-thickness <n>] [--bold|--no-bold] [--italic|--no-italic] [--underline|--no-underline]
      [--text-size <size>] [--image <MediaEntity>] [--display-string <text>] [--apply] [--json]
                              change only the given keys of a style; an empty color clears it;
                              plan unless --apply"#,
    },
    Usage {
        path: "media set",
        group: 1,
        text: r#"  media set <MediaEntity> --file <path> [--apply] [--json]
                              replace a MediaEntity's image (PNG, JPEG, GIF or SVG, up to 10 MB);
                              plan unless --apply"#,
    },
    Usage {
        path: "menu item",
        group: 1,
        text: r#"  menu item add <Menu> <Title>=<Mashup> [--default] [--before <Title>] [--apply] [--json]
  menu item remove <Menu> <Title> [--apply] [--json]
                              add or remove a mashup item, keeping the effective menu in step;
                              plan unless --apply"#,
    },
    Usage {
        path: "state set",
        group: 1,
        text: r#"  state set <StateDefinition> --type numeric|string --state <spec>... [--apply] [--json]
                              replace a StateDefinition's states (the specs of new state);
                              plan unless --apply"#,
    },
    Usage {
        path: "new thing",
        group: 1,
        text: r#"  new thing <name> [--project <name>] [--description <text>] [--template <ThingTemplate>] [--shape <ThingShape>]...
      [--value-stream <ValueStream>] [--visibility <principal>]... [--apply] [--json]
                              create a ThingWorx Thing as a repository file; plan unless --apply"#,
    },
    Usage {
        path: "new thing-template",
        group: 1,
        text: r#"  new thing-template <name> [--project <name>] [--description <text>] [--base <ThingTemplate>] [--shape <ThingShape>]...
      [--visibility <principal>]... [--apply] [--json]
                              create a ThingWorx ThingTemplate as a repository file; plan unless --apply"#,
    },
    Usage {
        path: "new thing-shape",
        group: 1,
        text: r#"  new thing-shape <name> [--project <name>] [--description <text>] [--visibility <principal>]... [--apply] [--json]
                              create a ThingWorx ThingShape as a repository file; plan unless --apply"#,
    },
    Usage {
        path: "new data-shape",
        group: 1,
        text: r#"  new data-shape <name> --field <name:TYPE[:primary]>... [--project <name>] [--description <text>] [--base <DataShape>]
      [--visibility <principal>]... [--apply] [--json]
                              create a DataShape and its fields.json sidecar; plan unless --apply"#,
    },
    Usage {
        path: "new timer",
        group: 1,
        text: r#"  new timer <name> --run-as <user> [--project <name>] [--every <duration>] [--disabled] [--then <code>|--script <file.js>]
      [--description <text>] [--visibility <principal>]... [--apply] [--json]
                              create a Timer Thing and optional Timer subscription; plan unless --apply"#,
    },
    Usage {
        path: "new scheduler",
        group: 1,
        text: r#"  new scheduler <name> --run-as <user> [--project <name>] [--cron <quartz cron>] [--disabled] [--then <code>|--script <file.js>]
      [--description <text>] [--visibility <principal>]... [--apply] [--json]
                              create a Scheduler Thing and optional ScheduledEvent subscription; plan unless --apply"#,
    },
    Usage {
        path: "new data-table",
        group: 1,
        text: r#"  new data-table <name> --data-shape <DataShape> [--project <name>] [--description <text>]
      [--index name=field1,field2]... [--apply] [--json]
                              create a persistent DataTable and datatable.json; plan unless --apply"#,
    },
    Usage {
        path: "new stream",
        group: 1,
        text: r#"  new stream <name> --data-shape <DataShape> [--project <name>] [--description <text>] [--apply] [--json]
                              create a persistent Stream; plan unless --apply"#,
    },
    Usage {
        path: "new value-stream",
        group: 1,
        text: r#"  new value-stream <name> [--project <name>] [--description <text>] [--apply] [--json]
                              create a persistent ValueStream; plan unless --apply"#,
    },
    Usage {
        path: "new media",
        group: 1,
        text: r#"  new media <name> --file <path> [--project <name>] [--description <text>] [--apply] [--json]
                              create a PNG, JPEG, GIF, or SVG MediaEntity; plan unless --apply"#,
    },
    Usage {
        path: "new style",
        group: 1,
        text: r#"  new style <name> [--project <name>] [--description <text>] [--background <color>] [--foreground <color>]
      [--secondary-background <color>] [--line-color <color>] [--line-style solid|dashed|dotted|none]
      [--line-thickness <n>] [--bold] [--italic] [--underline] [--text-size <size>]
      [--image <MediaEntity>] [--display-string <text>] [--apply] [--json]
                              create a StyleDefinition; plan unless --apply"#,
    },
    Usage {
        path: "new state",
        group: 1,
        text: r#"  new state <name> --type numeric|string --state <spec>... [--project <name>] [--description <text>]
      [--apply] [--json]
                              create a StateDefinition; plan unless --apply
      --state <spec>          <op><value>=<label>[;<style>:<value>]..., op <=, >=, ==, < or >
                              (string states == only), or the catch-all *=<label>; styles
                              background, foreground, secondary-background, line-color,
                              line-style, line-thickness, bold, italic, underline, text-size,
                              image, e.g. "<=10=Low;background:#00ff00""#,
    },
    Usage {
        path: "new menu",
        group: 1,
        text: r#"  new menu <name> --item <Title=Mashup>... [--project <name>] [--description <text>] [--default]
      [--label <text>] [--apply] [--json]
                              create a Menu; plan unless --apply"#,
    },
    Usage {
        path: "new theme",
        group: 1,
        text: r#"  new theme <name> [--project <name>] [--description <text>] [--from <StyleTheme>]
      [--prop name=value]... [--apply] [--json]
                              create a StyleTheme; plan unless --apply"#,
    },
    Usage {
        path: "new network",
        group: 1,
        text: r#"  new network <name> [--project <name>] [--description <text>] [--apply] [--json]
                              create an empty Network entity; plan unless --apply"#,
    },
    Usage {
        path: "network list",
        group: 2,
        text: r#"  network list [--json]       list repository Networks and their hierarchy counts"#,
    },
    Usage {
        path: "network show",
        group: 2,
        text: r#"  network show <Network> [<Thing>] [--depth <n>] [--server] [--profile <name>] [--json]
                              a Network's hierarchy as a tree: each Thing's template, things outside
                              the repository, nodes per template, problems; <Thing>: its subtree"#,
    },
    Usage {
        path: "network diff",
        group: 2,
        text: r#"  network diff <Network> [--profile <name>] [--json]
                              compare repository and server connections; exits 1 when they differ"#,
    },
    Usage {
        path: "network pull",
        group: 2,
        text: r#"  network pull <Network> [--profile <name>] [--apply] [--json]
                              copy server connections into the repository; plan unless --apply"#,
    },
    Usage {
        path: "network add",
        group: 2,
        text: r#"  network add <Network> <Thing> (--under <Thing> | --as-root) [--type <connectionType>]
      [--outside] [--apply] [--json]
                              add a Thing to a Network (a Thing has one parent); plan unless --apply
      --outside               a Thing the repository does not hold (it exists on the server)"#,
    },
    Usage {
        path: "network move",
        group: 2,
        text: r#"  network move <Network> <Thing> (--under <Thing> | --as-root) [--type <connectionType>]
      [--apply] [--json]      re-parent a Thing with its subtree; plan unless --apply"#,
    },
    Usage {
        path: "network remove",
        group: 2,
        text: r#"  network remove <Network> <Thing> [--subtree] [--apply] [--json]
                              remove a Network connection, never the Thing; plan unless --apply"#,
    },
    Usage {
        path: "network import",
        group: 2,
        text: r#"  network import <Network> --file <path> [--create-things] [--move] [--outside] [--apply] [--json]
                                map CSV assets into a Network hierarchy; plan unless --apply"#,
    },
    Usage {
        path: "entity status",
        group: 2,
        text: r#"  entity status [<entity>|--all] [--detail] [--record]
                              entity: full name, last dotted segment, or Collection/Name
      --record                 record matching working/server hashes as baseline"#,
    },
    Usage {
        path: "entity pull",
        group: 2,
        text: r#"  entity pull <entity>...|--all [--profile <name>] [--apply] [--json]
                              take server-changed entities, extract sidecars and record baselines"#,
    },
    Usage {
        path: "entity get",
        group: 2,
        text: r#"  entity get <entity>         fetch raw server XML to stdout or --out <path>
                              Collection/Name: any server entity; a bare name: the repository's
      [--profile <name>]      server profile (default: default)"#,
    },
    Usage {
        path: "entity push",
        group: 2,
        text: r#"  entity push <entity> [--apply] [--json]
                              import one entity, refusing if the server changed
      --apply                 actually push (without it, report what would happen)
      --force                 push over a server-side change; the server's copy is saved first
      --no-backup             with --force, do not save the server's copy first"#,
    },
    Usage {
        path: "entity delete",
        group: 2,
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
    Usage {
        path: "entity carry",
        group: 2,
        text: r#"  entity carry <old> <new>... copy run-time, design-time and visibility permissions of renamed entities
                              (pairs written Collection/Old Collection/New); principals follow the ledger
      --renamed               also every entity the rename ledger has not yet carried or deleted
      --apply                 write the differing permissions, read each back, mark the ledger
      --detail                also ask the platform for its own difference count
      --json                  {plan|applied, entities:[collection, old, new, status, kinds, error]}"#,
    },
    Usage {
        path: "entity restore",
        group: 2,
        text: r#"  entity restore [<set> [<entity>...]]  list backup sets, or plan importing one back
      --apply                 import the set's entities, confirming each on the server
      --json                  {sets|plan|applied, ...}"#,
    },
    Usage {
        path: "permissions init",
        group: 2,
        text: r#"  permissions init [--project <name>] [--from-helper] [--apply] [--json]
                              draft a permissions.toml for each project without one, from what it
                              grants today, so that `permissions apply` then changes nothing but
                              what its notes name; roles from the permission helper when there is
                              one; prints unless --apply
      --from-helper           take the grants from the helper's tables, not the entity XML (a
                              matrix edited in the helper's mashup)"#,
    },
    Usage {
        path: "requirements init",
        group: 2,
        text: r#"  requirements init [--apply] [--json]
                              draft requirements.toml from external repository dependencies;
                              prints a plan unless --apply; refuses an existing file"#,
    },
    Usage {
        path: "requirements check",
        group: 2,
        text: r#"  requirements check [--server [--profile <name>]] [--detail] [--json]
                              check requirements.toml against the repository; stale entries are notes
      --server                also verify platform, extensions and entities on the server, read-only
      --detail                every finding with its referrers"#,
    },
    Usage {
        path: "permissions audit",
        group: 2,
        text: r#"  permissions audit [--project <name>] [--server] [--detail] [--json]
                              each project's permissions.toml against its entity XML, offline: blocks
                              the policy would change, services a strict entity leaves unclassified,
                              principals the server refuses or no entity defines, rules that match
                              nothing; exit 1 on any error
      --server                also the server (read-only): each entity's permissions, the helper's
                              tables, the policy's platform grants and memberships, each role's unit
      --detail                every grant behind a finding"#,
    },
    Usage {
        path: "permissions apply",
        group: 2,
        text: r#"  permissions apply [--project <name>] [--apply] [--detail] [--json]
                              write each project's permissions.toml into its entity XML: the run-time
                              block of each Thing, the instance run-time block of each shape and
                              template, and the role principals of each visibility block; only blocks
                              that differ change; plan unless --apply; refused while a strict
                              entity has an unclassified service
      --detail                every grant each file gains or loses"#,
    },
    Usage {
        path: "permissions diff",
        group: 2,
        text: r#"  permissions diff <entity>...|--all [--json]
                              each entity's run-time, design-time and visibility permissions (and a
                              shape's or template's instance permissions) against the server's:
                              grants only the server has (an import never removes one), grants only
                              the repository has, and allow/deny that differs (an import keeps the
                              server's); exit 1 when any differs"#,
    },
    Usage {
        path: "permissions push",
        group: 2,
        text: r#"  permissions push <entity>...|--all|--platform [--apply] [--json]
                              make the server's permission sets exactly the repository's, set by set,
                              reading each back; plan unless --apply; records the baseline of pushed
                              entities that then match the server
      --platform              instead, add the policies' [[platform]] grants and memberships the
                              server lacks (what DeployComponent does); never removes anything"#,
    },
    Usage {
        path: "datatable copy",
        group: 2,
        text: r#"  datatable copy <old> <new> [--map a=b,c=d] [--drop-unmapped] [--append] [--max-rows <n>] [--apply] [--json]
                              copy a DataTable's rows into the one that replaced it; plan unless --apply
      --map a=b,...           source field -> target field (same names and the rename ledger match otherwise)
      --drop-unmapped         leave behind source fields that have no target field
      --append                allow a target that already has rows"#,
    },
    Usage {
        path: "bundle",
        group: 2,
        text: r#"  bundle [--backend-only] [--handoff <name>] [--apply]  write one importable document
                              (always written: --apply is accepted and changes nothing)
      --check                 report whether the bundle is current, write nothing"#,
    },
    Usage {
        path: "deploy",
        group: 2,
        text: r#"  deploy [--apply] [--force] [--json]
                              check, bundle, live-parse, conflict-check, import
      --backend-only          exclude configured UI collections
      --only <entity>         include only this entity (repeatable)
      --only-projects <a,b>   include only these projects
      --skip-checks           skip the gates, server requirements too; scripts are still parsed
      --no-backup             with --force, do not save the server's copies first"#,
    },
    Usage {
        path: "config-table",
        group: 2,
        text: r#"  config-table <thing> [<table>]  one Thing's configuration table on the server; without a
                              table, the Thing's tables
      --backup <file>         save it (never overwrites a file)
      --restore <file>        put a backup back; a plan unless --apply
      --diff                  compare with the entity XML in the repository
      --detail                every row, not only the first"#,
    },
    Usage {
        path: "db run",
        group: 2,
        text: r#"  db run <file.sql> [--thing <name>] [--no-transaction] [--timeout <seconds>] [--apply] [--json]
                              run one atomic SQLCommand; plan unless --apply"#,
    },
    Usage {
        path: "db query",
        group: 2,
        text: r#"  db query <file.sql>|-q <sql> [--thing <name>] [--max-rows <n>] [--timeout <seconds>] [--detail] [--json]
                              run a read-only SQLQuery; summary shows columns and first 20 rows.
                              Like db run, it imports a temporary ZZ.Twaco.Sql.* Thing and deletes it"#,
    },
    Usage {
        path: "db clean",
        group: 2,
        text: r#"  db clean [--apply] [--json] delete temporary ZZ.Twaco.Sql.* Things an interrupted db run left; plan unless --apply"#,
    },
    Usage {
        path: "call",
        group: 3,
        text: r#"  call <target> <service> [<json>] [--timeout <seconds>] [--detail] [--as <test user>]
      [--lang <code>]         target: Collection/Name, full name, or last dotted segment
      --with-logs             then what it wrote to ScriptLog and ApplicationLog (waits up to 3 s)
      --as <test user>        call as a [[test.user]] with the profile's test_password (the profile
                              needs allow_test_users): 404 if the user cannot see it, 401 a bad password
      --lang <code>           with --as: in this language (de, or fr,de: the first with a table);
                              the test user's language preference is set for the call, then put back"#,
    },
    Usage {
        path: "logs",
        group: 3,
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
    Usage {
        path: "settings",
        group: 3,
        text: r#"  settings                    the server's subsystems: running, and how many settings tables
  settings <Subsystem> [<Table>]  every setting of one, with its value and description
  settings --search <text>    settings whose name or description contains the text
                              (read-only; PASSWORD values are never shown); --json"#,
    },
    Usage {
        path: "repo",
        group: 4,
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
    Usage {
        path: "localization",
        group: 4,
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
    Usage {
        path: "users",
        group: 4,
        text: r#"  users list [--json]         the test users of [[test.user]] on the server: present, signing in
                              with the profile's test_password, in their groups
  users seed                  create the missing users, the [test] project and group memberships
  users delete                delete the test users; both need allow_test_users = true in the
                              profile and plan until --apply"#,
    },
    Usage {
        path: "appkey",
        group: 4,
        text: r#"  appkey create <name> --user <user> [--expires <30d|12h|date>] [--project <name>]
      [--description <text>] [--whitelist <ip,...>] [--save <profile>] [--show] [--apply] [--json]
  appkey show <name> [--save <profile>] [--show] [--json]
  appkey delete <name> [--apply]   as entity delete ApplicationKeys/<name>
                              an application key on the server, acting as a user; create plans
                              until --apply; the key is printed only with --show
      --user <user>           the user the key acts as
      --expires <when>        30d (the default), 12h, a date (midnight UTC) or a timestamp; without
                              one the server would expire it after a day
      --whitelist <ip,...>    addresses or ranges it may be used from (default any)
      --save <profile>        write it as app_key of that profile file, which must exist
      --show                  print the key"#,
    },
    Usage {
        path: "test",
        group: 4,
        text: r#"  test [<filter>...]          the service cases of tests/*.toml: each called as each of its test
                              users, expecting a status and optionally rows; checks and lists them
                              until --apply runs them (a case may change the server; the profile
                              must say allow_test_users = true); exit 1 when one fails
      --detail                every passing call too
      --json                  the calls run as JSON"#,
    },
    Usage {
        path: "ext",
        group: 4,
        text: r#"  ext list [--json]           the server's extension packages
  ext show <package>          one package: its extensions, and which are in use
  ext import <zip>            validate a package on the server; --apply installs it
  ext remove <package>        plan removing a package (refused while in use); --apply removes it"#,
    },
    Usage {
        path: "search",
        group: 4,
        text: r#"  search [<text>] [--type <T>[,<T>]] [--project P] [--limit <n>] [--json]
                              the server's entity search (Composer's Spotlight): text
                              in a name or description, or a pattern with *"#,
    },
    Usage {
        path: "export",
        group: 4,
        text: r#"  export entity <Coll/Name> --out <file>   one entity's XML, from the server's Exporter
  export collection <Coll> [--project P] --out <file>   a collection, or one project's part of it
  export project <P> --out <file>  everything of a project, as one XML
      --force                 replace the --out file if it exists
  export source-control --repository R --path <p> [--project P] [--collection C] [--tags t]
      [--zip <name>] [--with-dependents]  the source-control layout into a repository; --apply sends it"#,
    },
    Usage {
        path: "import",
        group: 4,
        text: r#"  import <file.xml|.zip>      import an export into the server: a plan of what it adds and
                              replaces unless --apply
  import source-control --repository R --path <p>  a source-control tree from a repository;
                              the plan is the server's diff; --apply imports, then diffs again
      --detail                every entity of the plan, not only the counts
      --overwrite-properties --overwrite-tables  replace the server's property values and
                              configuration table rows (kept by default, as in Composer)"#,
    },
    Usage {
        path: "package",
        group: 5,
        text: r#"  package bundle [--project P] [--backend-only|--frontend-only] --out <file>
                              the repository as one importable XML, offline
  package source-control [--project P] --out <file.zip>  the <Project>/<Collection>/<Name>.xml layout
  package extension [--project P] [--editable] --out <file.zip>  an extension package: one project's,
                              or the solution's as a zip of its projects' ([package] in twaco.toml)
      --force                 replace the --out file if it exists"#,
    },
    Usage {
        path: "guide",
        group: 6,
        text: r#"  guide                       the knowledge topics: twaco's workflow, the platform's verified quirks,
                              the service-code reference, and the solution's own markdown
  guide <topic> [--section <heading>]  read one (a long one gives its outline)
  guide --search <words>      the sections that best match; --limit <n>; --json"#,
    },
    Usage {
        path: "help",
        group: 6,
        text: r#"  help search <words>         search the ThingWorx Platform help (the server's version)
  help page <page>            read a help page as Markdown; --section <heading>
      --version <10.1>        another release; --limit <n>; --refresh; --json
      --profile <name>        the server whose version to read (default: default)"#,
    },
    Usage {
        path: "javadoc",
        group: 6,
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
