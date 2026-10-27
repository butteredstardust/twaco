//! twaco: a portable ThingWorx sidecar toolchain.
//!
//! This is an adapter. Every decision lives in `twaco::core`, so the MCP server (`twaco::mcp`)
//! answers the same questions the same way rather than growing its own opinions.
//!
//! **Exit codes are three, not two.** 0 is success, 1 is "there is work to do" from a `--check`,
//! and 2 is a failure: bad arguments, bad configuration, or a file that would not read or write.
//! Automation has to be able to tell drift from breakage, and one code for both hides an outage
//! behind a diff.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use twaco::core::commands::{self};
use twaco::core::config::Solution;
use twaco::core::workspace;

mod cli;

use clap::FromArgMatches as _;
use cli::appkey::appkey_cmd;
use cli::command::Command;
use cli::content::{
    export_cmd, ext_add_cmd, ext_check_cmd, ext_cmd, ext_intellij_cmd, ext_new_cmd, import_cmd,
    package_cmd, repo_cmd, search_cmd,
};
use cli::data::{
    call, config_table, datatable_copy_cmd, db_clean_cmd, db_cmd, logs_cmd, Datatable, Db,
};
use cli::entity::{
    entity_carry_cmd, entity_delete_cmd, entity_get, entity_pull, entity_push, entity_restore_cmd,
    entity_status, permissions_apply_cmd, permissions_audit_cmd, permissions_cmd,
    permissions_init_cmd, Entity, Permissions,
};
use cli::entity_edit::entity_edit_cmd;
use cli::entity_new::{entity_new_cmd, New};
use cli::info::{
    catalog_cmd, docs_cmd, guide_cmd, help_cmd, impact_cmd, javadoc_cmd, settings_cmd, unused_cmd,
    update_cmd, write_agent_files,
};
use cli::localization::localization_cmd;
use cli::mashup::{
    mashup_add_cmd, mashup_add_contained_cmd, mashup_bind_cmd, mashup_describe_cmd,
    mashup_event_cmd, mashup_new_cmd, mashup_on_cmd, mashup_param_cmd, mashup_recipe_cmd,
    mashup_templates_cmd, mashup_validate_cmd, Mashup,
};
use cli::network::network_cmd;
use cli::refactor::{
    adopt_cmd, handoff_cmd, new_building_block_cmd, relocate_cmd, rename_cmd, reproject_cmd,
    retemplate_cmd, CopyMember, MoveMember,
};
use cli::service_tests::test_cmd;
use cli::source::{bundle, check, deploy_cmd, extract, fmt, sync_cmd, types_cmd};
use cli::users::users_cmd;
use cli::widgets::widgets_cmd;

/// Success.
const OK: u8 = 0;
/// A `--check` found work to do. Not an error.
const DRIFT: u8 = 1;
/// Something failed: arguments, configuration, or I/O.
const FAILED: u8 = 2;

fn main() -> ExitCode {
    quiet_when_the_reader_leaves();
    twaco::core::diagnostics::set_notice_sink(print_notice);
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = run_command(&args);
    update_notice(args.first().map(String::as_str).unwrap_or_default());
    code
}

/// A warning core raised for the user, on stderr: the MCP server's stdout is JSON-RPC.
fn print_notice(message: &str) {
    eprintln!("{} warning: {message}", cli::style::prefix());
}

/// After the command, a line on stderr when a newer release exists. The network is asked at
/// most once a day, with a short timeout, and never for the MCP server, CI or a redirected
/// stderr. See core::update.
fn update_notice(command: &str) {
    use std::io::IsTerminal;
    use twaco::core::update;
    let wanted = update::notice_wanted(
        command,
        &|name| std::env::var(name).ok(),
        std::io::stderr().is_terminal(),
    );
    if command.is_empty() || !wanted {
        return;
    }
    let Some(cache) = update::cache_file() else {
        return;
    };
    let web = update::Web::new(Duration::from_secs(2));
    let now = jiff::Timestamp::now().as_second();
    let current = env!("CARGO_PKG_VERSION");
    if let Some(line) = update::notice(
        &web,
        update::MANIFEST_URL,
        update::PUBLIC_KEY,
        &cache,
        now,
        current,
    ) {
        eprintln!("{} {line}", cli::style::prefix());
    }
}

fn run_command(args: &[String]) -> ExitCode {
    // clap refuses what no command takes, with a suggestion; it answers --help and --version
    // (exit 0) and a usage error (exit 2, FAILED) itself.
    let words = std::iter::once("twaco".to_string()).chain(args.iter().cloned());
    let matches = match cli::spec::tree().try_get_matches_from(words) {
        Ok(matches) => matches,
        Err(error) => {
            // Every twaco error starts `twaco:`, which people and scripts look for; help,
            // --version and the listing for a missing command are not errors and stay as clap
            // writes them.
            let plain = matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp
                    | clap::error::ErrorKind::DisplayVersion
                    | clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
            );
            if plain {
                let _ = error.print();
            } else {
                let text = error.render().to_string();
                eprint!(
                    "{} {}",
                    cli::style::prefix(),
                    text.strip_prefix("error: ").unwrap_or(&text)
                );
            }
            return ExitCode::from(u8::try_from(error.exit_code()).unwrap_or(FAILED));
        }
    };
    let (_, matched) = cli::spec::matched(&matches);
    // The command's struct, and what clap cannot check refused here, before the solution is
    // looked for.
    let parsed = Command::from_arg_matches(&matches)
        .map_err(|error| error.to_string())
        .and_then(|command| command.validate().map(|()| command));
    let command = match parsed {
        Ok(command) => command,
        Err(why) => {
            eprintln!("{} {why}", cli::style::prefix());
            return ExitCode::from(FAILED);
        }
    };
    let (log, log_file) = cli::spec::log_options(matched);
    if let Some(warning) = twaco::core::diagnostics::init(log.as_deref(), log_file.as_deref()) {
        eprintln!("{} {warning}", cli::style::prefix());
    }
    let span = tracing::info_span!("command", command = command.path());
    let _entered = span.enter();
    let started = std::time::Instant::now();
    let code = dispatch(command);
    tracing::info!(
        exit = code,
        elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        "command finished"
    );
    ExitCode::from(code)
}

/// Run one command and return its exit code.
fn dispatch(command: Command) -> u8 {
    match command {
        // update replaces the binary, not the solution, so it needs none.
        Command::Update(update) => update_cmd(&update),
        // init makes the config discovery would look for, so it runs before discovery too.
        Command::Init(init) => init_cmd(&init),
        // doctor diagnoses a missing solution rather than failing on it.
        Command::Doctor(doctor) => doctor_cmd(&doctor),
        Command::Mcp(mcp) => mcp_cmd(&mcp),
        // Completions describe the command line, not a solution.
        Command::Completions(completions) => completions_cmd(&completions),
        Command::Projects(projects) => run(|solution| projects_cmd(solution, &projects)),
        // The help center needs no solution, so it runs before discovery and uses one if found.
        Command::Help(help) => help_cmd(&help),
        // The public Java API documentation also needs no solution.
        Command::Javadoc(javadoc) => javadoc_cmd(&javadoc),
        // So does the guide: its built-in topics are the same everywhere.
        Command::Guide(guide) => guide_cmd(&guide),
        Command::Package(package) => run(|solution| package_cmd(solution, &package)),
        Command::Widgets(widgets) => widgets_cmd(&widgets),
        Command::Catalog(catalog) => run(|solution| catalog_cmd(solution, &catalog)),
        Command::Contract(cli::contract::Contract::Diff(args)) => {
            run(|solution| cli::contract::diff_cmd(solution, &args))
        }
        Command::Impact(impact) => run(|solution| impact_cmd(solution, &impact)),
        Command::Trace(trace) => run(|solution| cli::info::trace_cmd(solution, &trace)),
        Command::Unused(unused) => run(|solution| unused_cmd(solution, &unused)),
        Command::Docs(docs) => run(|solution| docs_cmd(solution, &docs)),
        Command::Types(types) => run(|solution| types_cmd(solution, &types)),
        Command::Extract(command) => run(|solution| extract(solution, &command)),
        Command::Sync(sync) => run(|solution| sync_cmd(solution, &sync)),
        Command::Fmt(command) => run(|solution| fmt(solution, &command)),
        Command::Check(command) => run(|solution| check(solution, &command)),
        Command::Users(users) => run(|solution| users_cmd(solution, &users)),
        Command::AppKey(appkey) => run(|solution| appkey_cmd(solution, &appkey)),
        Command::Test(test) => run(|solution| test_cmd(solution, &test)),
        Command::Localization(localization) => {
            run(|solution| localization_cmd(solution, &localization))
        }
        Command::Repo(repo) => run(|solution| repo_cmd(solution, &repo)),
        Command::Block(block) => run(|solution| cli::block::block_cmd(solution, &block)),
        // An extension project is not a solution: `ext check` needs no twaco.toml.
        Command::Ext(ext) if ext.operands.first().map(String::as_str) == Some("check") => {
            ext_check_cmd(&ext)
        }
        Command::Ext(ext) if ext.operands.first().map(String::as_str) == Some("new") => {
            ext_new_cmd(&ext)
        }
        Command::Ext(ext) if ext.operands.first().map(String::as_str) == Some("add") => {
            ext_add_cmd(&ext)
        }
        Command::Ext(ext) if ext.operands.first().map(String::as_str) == Some("intellij") => {
            ext_intellij_cmd(&ext)
        }
        Command::Ext(ext) => run(|solution| ext_cmd(solution, &ext)),
        Command::Requirements(command) => {
            run(|solution| cli::requirements::requirements_cmd(solution, &command))
        }
        Command::Search(search) => run(|solution| search_cmd(solution, &search)),
        Command::Export(export) => run(|solution| export_cmd(solution, &export)),
        Command::Import(import) => run(|solution| import_cmd(solution, &import)),
        Command::Call(command) => run(|solution| call(solution, &command)),
        Command::Logs(logs) => run(|solution| logs_cmd(solution, &logs)),
        Command::Settings(settings) => run(|solution| settings_cmd(solution, &settings)),
        Command::Thing(thing) => run(|solution| entity_edit_cmd(solution, thing.edit())),
        Command::Timer(timer) => run(|solution| entity_edit_cmd(solution, timer.edit())),
        Command::Scheduler(scheduler) => {
            run(|solution| entity_edit_cmd(solution, scheduler.edit()))
        }
        Command::Style(style) => run(|solution| entity_edit_cmd(solution, style.edit())),
        Command::Media(media) => run(|solution| entity_edit_cmd(solution, media.edit())),
        Command::Menu(menu) => run(|solution| entity_edit_cmd(solution, menu.edit())),
        Command::State(state) => run(|solution| entity_edit_cmd(solution, state.edit())),
        Command::McpNamespace(namespace) => {
            run(|solution| entity_edit_cmd(solution, namespace.edit()))
        }
        Command::Agent(agent) => run(|solution| entity_edit_cmd(solution, agent.edit())),
        Command::Network(network) => run(|solution| network_cmd(solution, &network)),
        Command::New(New::BuildingBlock(block)) => {
            run(|solution| new_building_block_cmd(solution, &block))
        }
        Command::New(New::Entity(new)) => run(|solution| entity_new_cmd(solution, &new)),
        // The templates are built in, so they need no solution.
        Command::Mashup(Mashup::Templates(templates)) => mashup_templates_cmd(&templates),
        Command::Mashup(mashup) => run(|solution| match &mashup {
            Mashup::Add(add) => mashup_add_cmd(solution, add),
            Mashup::AddContained(add) => mashup_add_contained_cmd(solution, add),
            Mashup::Bind(bind) => mashup_bind_cmd(solution, bind),
            Mashup::On(on) => mashup_on_cmd(solution, on),
            Mashup::Param(param) => mashup_param_cmd(solution, param),
            Mashup::Event(event) => mashup_event_cmd(solution, event),
            Mashup::Recipe(recipe) => mashup_recipe_cmd(solution, recipe),
            Mashup::New(new) => mashup_new_cmd(solution, new),
            Mashup::Describe(describe) => mashup_describe_cmd(solution, describe),
            Mashup::Validate(validate) => mashup_validate_cmd(solution, validate),
            Mashup::Templates(_) => unreachable!("the templates need no solution"),
        }),
        Command::Adopt(adopt) => run(|solution| adopt_cmd(solution, &adopt)),
        Command::Handoff(handoff) => run(|solution| handoff_cmd(solution, &handoff)),
        Command::Rename(rename) => run(|solution| rename_cmd(solution, &rename)),
        Command::Move(relocate) => run(|solution| match &relocate {
            MoveMember::Service(service) => relocate_cmd(
                solution,
                relocate.path(),
                &service.relocate,
                service.leave_delegate,
            ),
            MoveMember::Property(property) => {
                relocate_cmd(solution, relocate.path(), property, false)
            }
        }),
        Command::Copy(relocate) => run(|solution| match &relocate {
            CopyMember::Service(member) | CopyMember::Property(member) => {
                relocate_cmd(solution, relocate.path(), member, false)
            }
        }),
        Command::Retemplate(retemplate) => run(|solution| retemplate_cmd(solution, &retemplate)),
        Command::Reproject(reproject) => run(|solution| reproject_cmd(solution, &reproject)),
        Command::Entity(entity) => run(|solution| match &entity {
            Entity::Get(get) => entity_get(solution, get),
            Entity::Status(status) => entity_status(solution, status),
            Entity::Pull(pull) => entity_pull(solution, pull),
            Entity::Push(push) => entity_push(solution, push),
            Entity::Delete(delete) => entity_delete_cmd(solution, delete),
            Entity::Carry(carry) => entity_carry_cmd(solution, carry),
            Entity::Restore(restore) => entity_restore_cmd(solution, restore),
        }),
        Command::Permissions(permissions) => run(|solution| match &permissions {
            Permissions::Diff(diff) => permissions_cmd(solution, diff, None),
            Permissions::Push(push) => permissions_cmd(solution, &push.diff, Some(push)),
            Permissions::Audit(audit) => permissions_audit_cmd(solution, audit),
            Permissions::Apply(apply) => permissions_apply_cmd(solution, apply),
            Permissions::Init(init) => permissions_init_cmd(solution, init),
        }),
        Command::Bundle(command) => run(|solution| bundle(solution, &command)),
        Command::Deploy(deploy) => run(|solution| deploy_cmd(solution, &deploy)),
        Command::ConfigTable(table) => run(|solution| config_table(solution, &table)),
        Command::Datatable(Datatable::Copy(copy)) => {
            run(|solution| datatable_copy_cmd(solution, &copy))
        }
        Command::Db(db) => run(|solution| match &db {
            Db::Run(run) => db_cmd(solution, &run.line()),
            Db::Query(query) => db_cmd(solution, &query.line()),
            Db::Clean(clean) => db_clean_cmd(solution, clean),
        }),
    }
}

/// `twaco init [--write|--agents]`.
#[derive(Debug, clap::Args)]
#[group(skip)]
struct Init {
    #[arg(long = "write")]
    write: bool,
    #[arg(long = "agents")]
    agents: bool,
    #[arg(long = "from-server", value_name = "project", num_args = 1..)]
    from_server: Vec<String>,
    #[command(flatten)]
    profile: cli::flags::Profile,
    #[command(flatten)]
    apply: cli::flags::Apply,
    #[command(flatten)]
    json: cli::flags::Json,
}

/// `twaco init [--write|--agents]`: propose a twaco.toml from the repository's own entities.
fn init_cmd(parsed: &Init) -> u8 {
    let here = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if !parsed.from_server.is_empty() {
        if parsed.write || parsed.agents {
            eprintln!(
                "{} init --from-server does not take --write or --agents",
                cli::style::prefix()
            );
            return FAILED;
        }
        let result = twaco::core::commands::init_server::init_from_server(
            twaco::core::commands::init_server::InitFromServerRequest {
                root: here.clone(),
                projects: parsed.from_server.clone(),
                profile: parsed.profile.name().to_string(),
                apply: parsed.apply.apply,
            },
        );
        return match result {
            Ok(outcome) if parsed.json.json => {
                println!(
                    "{}",
                    twaco::core::commands::init_server::outcome_json(&outcome)
                );
                if outcome.result.not_in_sync.is_empty() {
                    OK
                } else {
                    DRIFT
                }
            }
            Ok(outcome) => {
                let outcome = outcome.result;
                println!(
                    "{} project(s), {} entity file(s){}",
                    outcome.projects.len(),
                    outcome.entities.values().sum::<usize>(),
                    if outcome.applied {
                        if outcome.added {
                            " added"
                        } else {
                            " written"
                        }
                    } else {
                        " planned"
                    }
                );
                for (collection, count) in &outcome.entities {
                    println!("{collection}: {count}");
                }
                print!("{}", outcome.toml);
                for note in outcome.notes {
                    eprintln!("{} {note}", cli::style::prefix());
                }
                if !outcome.applied {
                    eprintln!(
                        "{} nothing written; pass --apply to create the repository",
                        cli::style::prefix()
                    );
                }
                if outcome.applied {
                    println!("baseline recorded for {} entities", outcome.recorded);
                    println!(
                        "localization: {} file(s) written, {} token(s) pulled",
                        outcome.localization_files, outcome.localization_tokens
                    );
                    for entity in &outcome.not_in_sync {
                        eprintln!("{} not in sync: {entity}", cli::style::prefix());
                    }
                }
                if outcome.not_in_sync.is_empty() {
                    OK
                } else {
                    DRIFT
                }
            }
            Err(error) => {
                eprintln!("{} {error}", cli::style::prefix());
                FAILED
            }
        };
    }
    if parsed.apply.apply || parsed.json.json || parsed.profile.profile.is_some() {
        eprintln!(
            "{} --apply, --json and --profile require --from-server",
            cli::style::prefix()
        );
        return FAILED;
    }
    let write = match (parsed.write, parsed.agents) {
        (false, false) => false,
        (true, false) => true,
        // The agent files alone, for a solution that already has its config.
        (false, true) => {
            return match Solution::discover(&here) {
                Ok(solution) => write_agent_files(&solution),
                Err(error) => {
                    eprintln!("{} {error}", cli::style::prefix());
                    FAILED
                }
            };
        }
        (true, true) => {
            eprintln!(
                "{} init takes --write or --agents, not both",
                cli::style::prefix()
            );
            return FAILED;
        }
    };
    let proposal = twaco::core::init::propose(&here);
    for note in &proposal.notes {
        eprintln!("{} {note}", cli::style::prefix());
    }
    if proposal.projects == 0 {
        return FAILED;
    }
    let target = here.join(twaco::core::config::CONFIG_FILE);
    if !write {
        print!("{}", proposal.toml);
        eprintln!("{} nothing written; `twaco init --write` creates {}, and AGENTS.md and CLAUDE.md where absent", cli::style::prefix(), target.display());
        return OK;
    }
    // Created new, never renamed over: whatever is at the name, a file or a link, stays.
    let created = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)
        .and_then(|mut file| std::io::Write::write_all(&mut file, proposal.toml.as_bytes()));
    match created {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            eprintln!(
                "{} {} exists and is never overwritten; remove it first to start again",
                cli::style::prefix(),
                target.display()
            );
            return FAILED;
        }
        Err(error) => {
            eprintln!("{} {}: {error}", cli::style::prefix(), target.display());
            return FAILED;
        }
    }
    // Prove it loads, so a proposal twaco cannot read is never left behind silently.
    match Solution::load(&target) {
        Ok(solution) => {
            println!(
                "wrote {} with {} project(s); `twaco doctor` checks the rest",
                target.display(),
                solution.projects.len()
            );
            write_agent_files(&solution)
        }
        Err(error) => {
            eprintln!(
                "{} the written config does not load: {error}",
                cli::style::prefix()
            );
            FAILED
        }
    }
}

/// `twaco doctor [--profile <name>]`.
#[derive(Debug, clap::Args)]
#[group(skip)]
struct Doctor {
    #[command(flatten)]
    profile: cli::flags::Profile,
    #[command(flatten)]
    json: cli::flags::Json,
}

/// `twaco doctor [--profile <name>]`: what resolved, what is reachable, what is missing.
fn doctor_cmd(parsed: &Doctor) -> u8 {
    let here = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let items = twaco::core::doctor::diagnose(&here, parsed.profile.name());
    if parsed.json.json {
        println!("{}", twaco::core::doctor::report_json(&items));
        return if items
            .iter()
            .any(|item| item.health == twaco::core::doctor::Health::Fail)
        {
            FAILED
        } else {
            OK
        };
    }
    for item in &items {
        let mark = match item.health {
            twaco::core::doctor::Health::Ok => "ok  ",
            twaco::core::doctor::Health::Warn => "warn",
            twaco::core::doctor::Health::Fail => "FAIL",
        };
        println!("  {mark}  {:<15} {}", item.subject, item.detail);
    }
    let failed = items
        .iter()
        .any(|i| i.health == twaco::core::doctor::Health::Fail);
    if failed {
        FAILED
    } else {
        OK
    }
}

/// `twaco mcp [--tools <groups>]`.
#[derive(Debug, clap::Args)]
#[group(skip)]
struct Mcp {
    /// The tool groups to serve; `TWACO_MCP_TOOLS` when absent.
    #[arg(long = "tools", value_name = "groups")]
    tools: Option<String>,
}

/// The tool groups asked for: `--tools`, else `TWACO_MCP_TOOLS` unless it is blank, as client
/// configurations often template an unused variable to an empty string.
fn tool_groups(flag: Option<&str>, variable: Option<String>) -> Option<String> {
    flag.map(str::to_string)
        .or_else(|| variable.filter(|value| !value.trim().is_empty()))
}

/// `twaco mcp [--tools <groups>]`. The server finds the solution on every call rather than at
/// start, so it starts in a directory without one and each tool says so, and an edit to
/// twaco.toml needs no restart. The tool groups are fixed for the server's life: it tells clients
/// its list does not change.
fn mcp_cmd(parsed: &Mcp) -> u8 {
    let root = std::env::var_os("TWACO_ROOT")
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    let groups = tool_groups(
        parsed.tools.as_deref(),
        std::env::var("TWACO_MCP_TOOLS").ok(),
    );
    let selection = match groups.as_deref().map(twaco::mcp::Selection::parse) {
        None => twaco::mcp::Selection::ALL,
        Some(Ok(selection)) => selection,
        Some(Err(why)) => {
            eprintln!("{} mcp: {why}", cli::style::prefix());
            return FAILED;
        }
    };
    let stdin = std::io::stdin();
    match twaco::mcp::serve_tools(&root, &selection, stdin.lock(), std::io::stdout()) {
        Ok(()) => OK,
        Err(error) => {
            eprintln!("{} mcp: {error}", cli::style::prefix());
            FAILED
        }
    }
}

/// `twaco completions <shell>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
struct Completions {
    /// The shell to write the script for.
    #[arg(id = "shell", value_name = "shell", value_enum)]
    shell: Option<clap_complete::Shell>,
}

/// `twaco completions <shell>`: the completion script for one shell, on stdout, from the tree
/// twaco parses with, so it offers exactly the commands, flags and actions twaco takes.
fn completions_cmd(parsed: &Completions) -> u8 {
    let Some(shell) = parsed.shell else {
        eprintln!(
            "{} completions needs a shell: bash, elvish, fish, powershell or zsh",
            cli::style::prefix()
        );
        return FAILED;
    };
    // Written whole through print!, so a reader that leaves early is quiet, as for any output.
    let mut tree = cli::spec::tree();
    if shell == clap_complete::Shell::Zsh {
        tree = escape_colons_for_zsh(tree);
    }
    let mut script = Vec::new();
    clap_complete::generate(shell, &mut tree, "twaco", &mut script);
    print!("{}", String::from_utf8_lossy(&script));
    OK
}

/// zsh's `_arguments` splits a spec at `:`, and clap_complete escapes it in help text but not in
/// value names such as `name:TYPE[:DIRECTION]`, so those would break completion of the flag.
fn escape_colons_for_zsh(command: clap::Command) -> clap::Command {
    let command = command.mut_subcommands(escape_colons_for_zsh);
    let ids: Vec<clap::Id> = command
        .get_arguments()
        .filter(|arg| {
            arg.get_value_names()
                .is_some_and(|names| names.iter().any(|name| name.contains(':')))
        })
        .map(|arg| arg.get_id().clone())
        .collect();
    ids.iter().fold(command, |command, id| {
        command.mut_arg(id, |arg| {
            // Leaked: clap takes `'static` names, and this runs once before the process ends.
            let names: Vec<&'static str> = arg
                .get_value_names()
                .unwrap_or_default()
                .iter()
                .map(|name| &*Box::leak(name.replace(':', "\\:").into_boxed_str()))
                .collect();
            arg.value_names(names)
        })
    })
}

/// Exit quietly when stdout's reader has gone, as `twaco projects | head` does once it has its
/// lines. `println!` panics on a closed pipe, and a panic there is noise. The exit is 141, what
/// a shell reports for a process its pipe closed: the command's own result is unknown at this
/// point, so it must not read as success.
///
/// This reads std's panic message, which std does not promise to keep. The alternatives cost
/// more: restoring `SIGPIPE` needs the crate's first `unsafe` and covers only Unix, and fallible
/// writes would replace some 800 `println!`s. `offline.rs`'s closed-pipe test runs on all three
/// CI systems, so a change in the message fails there first.
fn quiet_when_the_reader_leaves() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info.payload();
        let message = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or_default();
        // Broken pipe: os error 32 on Unix; ERROR_NO_DATA (232) or ERROR_BROKEN_PIPE (109) on
        // Windows.
        let closed = ["(os error 32)", "(os error 232)", "(os error 109)"]
            .iter()
            .any(|code| message.contains(code));
        if message.starts_with("failed printing to stdout") && closed {
            std::process::exit(141);
        }
        default(info);
    }));
}

/// What an executor reported about taking the workspace lock: files it swept and operations it
/// recovered, one line each on stderr.
fn print_notices(notices: &commands::Notices) {
    for line in notices.lines() {
        eprintln!("{} {line}", cli::style::prefix());
    }
}

/// Load the solution once, then hand it to a command.
fn run(command: impl FnOnce(&Solution) -> u8) -> u8 {
    let here = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match Solution::discover(&here) {
        Ok(solution) => command(&solution),
        Err(e) => {
            eprintln!("{} {e}", cli::style::prefix());
            FAILED
        }
    }
}

#[derive(Debug, clap::Args)]
#[group(skip)]
struct Projects {
    #[command(flatten)]
    json: cli::flags::Json,
}

fn projects_cmd(solution: &Solution, parsed: &Projects) -> u8 {
    if parsed.json.json {
        let report = match workspace::projects_json(solution) {
            Ok(report) => report,
            Err(error) => {
                eprintln!("{} {error}", cli::style::prefix());
                return FAILED;
            }
        };
        println!("{report}");
        return if report["unreadable"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
        {
            FAILED
        } else {
            OK
        };
    }
    let order = match solution.deploy_order() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{} {e}", cli::style::prefix());
            return FAILED;
        }
    };
    let label = if solution.solution.name.is_empty() {
        "(unnamed)"
    } else {
        &solution.solution.name
    };
    println!("solution {label}  root {}", solution.root.display());
    println!("sidecars {}", solution.src_root().display());
    println!();
    println!("deploy order:");
    for (index, project) in order.iter().enumerate() {
        let root = solution.project_root(project);
        let missing = if root.is_dir() { "" } else { "   [missing]" };
        println!("  {}. {}", index + 1, project.name);
        println!("       root {}{}", root.display(), missing);
        if !project.depends_on.is_empty() {
            println!("       after {}", project.depends_on.join(", "));
        }
    }

    let found = workspace::discover(solution);
    println!();
    println!("{} entity document(s)", found.entities.len());

    // An entity whose own projectName disagrees with the folder it sits in imports into the
    // wrong project and nothing else notices, so it is reported here, where a person would look.
    let misfiled: Vec<&workspace::EntityFile> =
        found.entities.iter().filter(|e| e.is_misfiled()).collect();
    if !misfiled.is_empty() {
        println!();
        println!("{} misfiled entity document(s):", misfiled.len());
        for entity in &misfiled {
            println!(
                "  {} says {} but is filed under {}",
                entity.info.name, entity.info.project, entity.found_under
            );
        }
    }
    if !found.unreadable.is_empty() {
        eprintln!();
        eprintln!("{} file(s) could not be read:", found.unreadable.len());
        for problem in &found.unreadable {
            eprintln!("  {problem}");
        }
        return FAILED;
    }
    OK
}
