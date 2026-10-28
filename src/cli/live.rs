//! Live questions to the server, and registering twaco with a coding assistant:
//! `access explain`, `model resolve`, `entity inspect` and `mcp configure`.

use super::flags::{Apply, Json, Profile};
use super::style;
use crate::{FAILED, OK};
use twaco::core::config::Solution;
use twaco::core::{access, mcp_config, model_query, profile, server};

/// `twaco access explain <user> <Collection/Name> <service> [--json]`.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum Access {
    Explain(AccessExplain),
}

#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct AccessExplain {
    #[command(flatten)]
    pub(crate) json: Json,
    #[command(flatten)]
    pub(crate) profile: Profile,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco model resolve <ThingTemplates|ThingShapes>/<name> [--limit <n>] [--json]`.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum Model {
    Resolve(ModelResolve),
}

#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct ModelResolve {
    #[arg(long = "limit", value_name = "n")]
    pub(crate) limit: Option<String>,
    #[command(flatten)]
    pub(crate) json: Json,
    #[command(flatten)]
    pub(crate) profile: Profile,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco entity inspect <Collection/Name> [--member <name>] [--values] [--json]`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct EntityInspect {
    #[arg(long = "member", value_name = "name")]
    pub(crate) member: Option<String>,
    #[arg(long = "values")]
    pub(crate) values: bool,
    #[command(flatten)]
    pub(crate) json: Json,
    #[command(flatten)]
    pub(crate) profile: Profile,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `Collection/Name`, as the live commands take an entity.
fn address(text: &str) -> Result<(String, String), String> {
    match text.split_once('/') {
        Some((collection, name)) if !collection.is_empty() && !name.is_empty() => {
            Ok((collection.to_string(), name.to_string()))
        }
        _ => Err(format!(
            "{text:?}: name the entity as Collection/Name, such as Things/Acme.Pump"
        )),
    }
}

fn client(solution: &Solution, profile: &Profile) -> Result<server::Client, String> {
    profile::load(&solution.root, profile.name())
        .map(server::Client::new)
        .map_err(|error| error.to_string())
}

fn fail(command: &str, why: impl std::fmt::Display) -> u8 {
    eprintln!("{} {command}: {why}", style::prefix());
    FAILED
}

fn print_json(value: &impl serde::Serialize) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).expect("JSON values serialise")
    );
}

pub(crate) fn access_cmd(solution: &Solution, args: &Access) -> u8 {
    let Access::Explain(args) = args;
    let [user, entity, service] = args.operands.as_slice() else {
        return fail("access explain", "needs <user> <Collection/Name> <service>");
    };
    let (collection, name) = match address(entity) {
        Ok(found) => found,
        Err(why) => return fail("access explain", why),
    };
    let remote = match client(solution, &args.profile) {
        Ok(remote) => remote,
        Err(why) => return fail("access explain", why),
    };
    let explanation = match access::explain(&remote, user, &collection, &name, service) {
        Ok(explanation) => explanation,
        Err(why) => return fail("access explain", why),
    };
    if args.json.json {
        print_json(&explanation);
    } else {
        println!(
            "{}: {} may{} invoke {} on {}",
            explanation.verdict.word(),
            explanation.user,
            match explanation.verdict {
                access::Verdict::Allowed => "",
                access::Verdict::Denied => " not",
                access::Verdict::Unknown => " or may not",
            },
            explanation.service,
            explanation.entity
        );
        for reason in &explanation.reasons {
            println!("  {reason}");
        }
    }
    // An answer, whatever it is, is a success; only a failure to get one is not.
    OK
}

pub(crate) fn model_cmd(solution: &Solution, args: &Model) -> u8 {
    let Model::Resolve(args) = args;
    let [entity] = args.operands.as_slice() else {
        return fail("model resolve", "needs <ThingTemplates|ThingShapes>/<name>");
    };
    let limit = match args.limit.as_deref().map(str::parse::<usize>) {
        None => model_query::DEFAULT_LIMIT,
        Some(Ok(limit)) => limit,
        Some(Err(_)) => return fail("model resolve", "--limit takes a number"),
    };
    let (collection, name) = match address(entity) {
        Ok(found) => found,
        Err(why) => return fail("model resolve", why),
    };
    let remote = match client(solution, &args.profile) {
        Ok(remote) => remote,
        Err(why) => return fail("model resolve", why),
    };
    let found = match model_query::resolve(&remote, &collection, &name, limit) {
        Ok(found) => found,
        Err(why) => return fail("model resolve", why),
    };
    if args.json.json {
        print_json(&found);
        return OK;
    }
    println!("{}: {} Thing(s) on the server", found.entity, found.total);
    for thing in &found.things {
        println!("  {thing}");
    }
    if found.truncated {
        println!(
            "  ... {} more; --limit <n> lists more",
            found.total - found.things.len() as u64
        );
    }
    OK
}

pub(crate) fn entity_inspect_cmd(solution: &Solution, args: &EntityInspect) -> u8 {
    let [entity] = args.operands.as_slice() else {
        return fail("entity inspect", "needs <Collection/Name>");
    };
    let (collection, name) = match address(entity) {
        Ok(found) => found,
        Err(why) => return fail("entity inspect", why),
    };
    let remote = match client(solution, &args.profile) {
        Ok(remote) => remote,
        Err(why) => return fail("entity inspect", why),
    };
    let found = match model_query::inspect(
        &remote,
        &collection,
        &name,
        args.member.as_deref(),
        args.values,
    ) {
        Ok(found) => found,
        Err(why) => return fail("entity inspect", why),
    };
    if args.json.json {
        print_json(&found);
        return OK;
    }
    println!(
        "{} on the server: {} member(s)",
        found.entity,
        found.members.len()
    );
    for (source, count) in &found.sources {
        println!("  {count:>4} from {source}");
    }
    let width = found
        .members
        .iter()
        .map(|m| m.name.len())
        .max()
        .unwrap_or(0);
    for member in &found.members {
        // A long value is cut here; --json gives it whole.
        let value = member
            .value
            .as_ref()
            .map(|value| {
                let text = value.to_string();
                match text.char_indices().nth(100) {
                    Some((at, _)) => format!(" = {}... (--json shows all of it)", &text[..at]),
                    None => format!(" = {text}"),
                }
            })
            .unwrap_or_default();
        println!(
            "  {:<8} {:<width$}  {} {}{}{value}",
            member.kind,
            member.name,
            member.source_type,
            member.source,
            member
                .base_type
                .as_ref()
                .map(|base| format!(" ({base})"))
                .unwrap_or_default(),
        );
    }
    if !found.unreadable.is_empty() {
        println!(
            "  not readable with this profile's rights: {}",
            found.unreadable.join(", ")
        );
    }
    OK
}

/// `twaco mcp configure <client> [--scope user|project] [--command <path>] [--replace] [--apply]`.
pub(crate) fn mcp_configure_cmd(
    solution_root: &std::path::Path,
    operands: &[String],
    scope: Option<&str>,
    command: Option<&str>,
    replace: bool,
    apply: &Apply,
    json: &Json,
) -> u8 {
    let [word] = operands else {
        return fail("mcp configure", "needs <codex|claude-code|cursor>");
    };
    let client = match mcp_config::Client::parse(word) {
        Ok(client) => client,
        Err(why) => return fail("mcp configure", why),
    };
    let scope = match scope.map(mcp_config::Scope::parse).transpose() {
        Ok(scope) => scope.unwrap_or(client.default_scope()),
        Err(why) => return fail("mcp configure", why),
    };
    let Some(home) = dirs::home_dir() else {
        return fail("mcp configure", "cannot find the home folder");
    };
    let program = match command {
        Some(command) => command.to_string(),
        None => match std::env::current_exe().and_then(dunce::canonicalize) {
            Ok(exe) => exe.to_string_lossy().into_owned(),
            Err(error) => {
                return fail(
                    "mcp configure",
                    format!("cannot find this executable: {error}"),
                )
            }
        },
    };
    // A project file is read by a client started in the project; a user-wide entry has to say
    // which solution it serves.
    let root = (scope == mcp_config::Scope::User)
        .then(|| dunce::canonicalize(solution_root).unwrap_or_else(|_| solution_root.to_path_buf()))
        .map(|root| root.to_string_lossy().into_owned());
    let entry = mcp_config::Entry {
        command: program,
        args: vec!["mcp".to_string()],
        root,
    };
    let plan = match mcp_config::plan(client, scope, &entry, solution_root, &home, replace) {
        Ok(plan) => plan,
        Err(why) => return fail("mcp configure", why),
    };
    let file = plan.file.as_ref().map(|path| path.display().to_string());
    let state = match &plan.change {
        mcp_config::Change::Add => "add",
        mcp_config::Change::Same => "same",
        mcp_config::Change::Differs { .. } => "differs",
    };
    let applied = if apply.apply && plan.writes() {
        match mcp_config::apply(&plan) {
            Ok(backup) => Some(backup),
            Err(why) => return fail("mcp configure", why),
        }
    } else {
        None
    };
    let refused = matches!(plan.change, mcp_config::Change::Differs { .. }) && !replace;
    if json.json {
        print_json(&serde_json::json!({
            "client": client.word(),
            "scope": scope.word(),
            "file": file,
            "change": state,
            "entry": plan.entry_text,
            "command": plan.command,
            "applied": applied.is_some(),
            "backup": applied.flatten().map(|path| path.display().to_string()),
            "refused": refused,
        }));
        return if refused { FAILED } else { OK };
    }
    if let Some(command) = &plan.command {
        println!(
            "{} keeps its user-wide servers itself; register twaco with:",
            client.word()
        );
        println!("  {command}");
        return OK;
    }
    let file = file.unwrap_or_default();
    match &plan.change {
        mcp_config::Change::Same => {
            println!("{file} already registers twaco as it would be; nothing to do");
            return OK;
        }
        mcp_config::Change::Differs { current } if !replace => {
            eprintln!(
                "{} mcp configure: {file} registers twaco differently (an MCP gateway may have wrapped it); --replace puts this entry in its place:\n{current}",
                style::prefix()
            );
            return FAILED;
        }
        _ => {}
    }
    match applied {
        Some(backup) => {
            println!(
                "{} twaco in {file}",
                if state == "add" {
                    "registered"
                } else {
                    "replaced"
                }
            );
            if let Some(backup) = backup {
                println!("  the old file is saved as {}", backup.display());
            }
            println!("  restart {} so it starts the server", client.word());
        }
        None => {
            println!(
                "would {} twaco in {file}:",
                if state == "add" {
                    "register"
                } else {
                    "replace"
                }
            );
            for line in plan.entry_text.lines() {
                println!("  {line}");
            }
            println!("run again with --apply to write it; the old file is saved beside it first");
        }
    }
    OK
}
