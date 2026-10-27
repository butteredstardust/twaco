//! `twaco block`: the solution's Solution Framework building blocks.

use super::flags::{Apply, Json, Profile};
use super::style;
use crate::{print_notices, FAILED, OK};
use serde_json::json;
use twaco::core::block::{self, Bump, VersionChange};
use twaco::core::commands::{self, Notices};
use twaco::core::config::Solution;
use twaco::core::server;

/// `twaco block list | show <block> | version <block>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Block {
    #[command(flatten)]
    apply: Apply,
    #[command(flatten)]
    json: Json,
    #[command(flatten)]
    profile: Profile,
    #[arg(long)]
    server: bool,
    #[arg(long = "bump", value_name = "patch|minor|major")]
    bump: Option<String>,
    #[arg(id = "operands", value_name = "operand")]
    operands: Vec<String>,
}

pub(crate) fn block_cmd(solution: &Solution, args: &Block) -> u8 {
    let fail = |message: String| {
        eprintln!("{} {message}", style::prefix());
        FAILED
    };
    let operands: Vec<&str> = args.operands.iter().map(String::as_str).collect();
    let action = operands.first().copied();
    if action != Some("version") && (args.bump.is_some() || args.apply.apply) {
        return fail("--bump and --apply are for block version".to_string());
    }
    match operands.as_slice() {
        ["list"] => list(solution, args),
        ["show", name] => show(solution, args, name),
        ["version", name] => version(solution, args, name, None),
        ["version", name, to] => version(solution, args, name, Some(to)),
        ["show" | "version"] => fail(format!(
            "block {} needs the block's name",
            action.unwrap_or_default()
        )),
        _ => fail("block takes list, show <block> or version <block>".to_string()),
    }
}

fn server_view(
    solution: &Solution,
    args: &Block,
) -> Result<Option<(server::Client, Vec<block::ServerBlock>)>, String> {
    if !args.server {
        return Ok(None);
    }
    commands::block::server(solution, args.profile.name(), server::Client::new).map(Some)
}

fn list(solution: &Solution, args: &Block) -> u8 {
    let blocks = block::blocks(solution);
    let server = match server_view(solution, args) {
        Ok(server) => server,
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    let compared = server
        .as_ref()
        .map(|(_, server)| block::compare(blocks.clone(), server));
    if args.json.json {
        let rows = match &compared {
            Some(compared) => serde_json::to_value(compared),
            None => serde_json::to_value(&blocks),
        }
        .expect("blocks serialise");
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "blocks": rows })).expect("block list JSON")
        );
        return OK;
    }
    if blocks.is_empty() {
        println!(
            "no building block: no project has a <project>.EntryPoint Thing with componentType"
        );
        return OK;
    }
    match compared {
        Some(compared) => {
            for item in &compared {
                let block = &item.block;
                let server = item.server.as_ref().map_or_else(
                    || "not on the server".to_string(),
                    |server| {
                        format!(
                            "server: {} {}{}",
                            or_none(&server.version),
                            or_none(&server.state),
                            server
                                .order
                                .map(|order| format!(", deploy order {order}"))
                                .unwrap_or_default()
                        )
                    },
                );
                println!(
                    "{}  {}  {}  {server}",
                    block.name,
                    or_none(&block.kind),
                    or_none(&block.version)
                );
                for note in &item.notes {
                    println!("  {} {note}", style::warn("note:"));
                }
            }
        }
        None => {
            for block in &blocks {
                println!(
                    "{}  {}  {}  {}",
                    block.name,
                    or_none(&block.kind),
                    or_none(&block.version),
                    block.display_name
                );
            }
        }
    }
    OK
}

fn or_none(text: &str) -> &str {
    if text.is_empty() {
        "(none)"
    } else {
        text
    }
}

fn show(solution: &Solution, args: &Block, name: &str) -> u8 {
    let block = match block::find(solution, name) {
        Ok(block) => block,
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    let server = match server_view(solution, args) {
        Ok(server) => server,
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    let on_server = server.as_ref().map(|(remote, server)| {
        let compared = block::compare(vec![block.clone()], server).remove(0);
        let parameters = compared
            .server
            .as_ref()
            .map(|_| block::deploy_parameters(remote, &block.entry_point));
        (compared, parameters)
    });
    if args.json.json {
        let mut value = serde_json::to_value(&block).expect("a block serialises");
        if let Some((compared, parameters)) = &on_server {
            value["server"] = serde_json::to_value(&compared.server).expect("serialises");
            value["notes"] = json!(compared.notes);
            match parameters {
                Some(Ok(parameters)) => value["deploy_parameters"] = json!(parameters),
                Some(Err(error)) => value["deploy_parameters_error"] = json!(error),
                None => {}
            }
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("block show JSON")
        );
        return OK;
    }
    println!("{}  ({})", block.name, block.file);
    let line = |label: &str, value: &str| {
        if !value.is_empty() {
            println!("  {label:<18}{value}");
        }
    };
    line("type", &block.kind);
    line("version", &block.version);
    if block.project_version.as_deref() != Some(block.version.as_str()) {
        line(
            "project version",
            block
                .project_version
                .as_deref()
                .unwrap_or("(no packageVersion)"),
        );
    }
    line("display name", &block.display_name);
    line("description", &block.description);
    line("entry point", &block.entry_point);
    line("template", &block.entry_point_template);
    line("manager", block.manager.as_deref().unwrap_or_default());
    line("main mashup", &block.main_mashup);
    line("organization", &block.default_organization);
    line("user group", &block.default_group);
    line(
        "permission helper",
        block.permission_helper.as_deref().unwrap_or_default(),
    );
    line("depends on", &block.dependencies.join(", "));
    if let Some((compared, parameters)) = on_server {
        match &compared.server {
            Some(server) => {
                println!(
                    "  {:<18}{} {}{}",
                    "on the server",
                    or_none(&server.version),
                    or_none(&server.state),
                    server
                        .order
                        .map(|order| format!(", deploy order {order}"))
                        .unwrap_or_default()
                );
            }
            None => println!("  {:<18}not there", "on the server"),
        }
        for note in &compared.notes {
            println!("  {} {note}", style::warn("note:"));
        }
        match parameters {
            Some(Ok(parameters)) if parameters.is_empty() => {
                println!("  deploy parameters: none");
            }
            Some(Ok(parameters)) => {
                println!("  deploy parameters (DeployComponent's deploymentConfig):");
                for parameter in parameters {
                    println!(
                        "    {} {}{}{}",
                        parameter.name,
                        parameter.types.join("|"),
                        if parameter.optional {
                            ", optional"
                        } else {
                            ", required"
                        },
                        if parameter.description.is_empty() {
                            String::new()
                        } else {
                            format!(": {}", parameter.description)
                        }
                    );
                }
            }
            Some(Err(error)) => println!("  {} {error}", style::warn("deploy parameters:")),
            None => {}
        }
    }
    OK
}

fn version(solution: &Solution, args: &Block, name: &str, to: Option<&str>) -> u8 {
    if args.server {
        eprintln!(
            "{} block version changes the repository only; --server has nothing to do",
            style::prefix()
        );
        return FAILED;
    }
    let change = match (&args.bump, to) {
        (Some(part), None) => match Bump::parse(part) {
            Some(bump) => VersionChange::Bump(bump),
            None => {
                eprintln!(
                    "{} --bump takes patch, minor or major, not {part:?}",
                    style::prefix()
                );
                return FAILED;
            }
        },
        (None, Some(version)) => VersionChange::Set(version.to_string()),
        _ => {
            eprintln!(
                "{} block version needs a version (x.y.z) or --bump patch|minor|major, not both",
                style::prefix()
            );
            return FAILED;
        }
    };
    let mut notices = Notices::default();
    let result = commands::block::version(solution, name, &change, args.apply.apply, &mut notices);
    print_notices(&notices);
    let plan = match result {
        Ok(plan) => plan,
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    if args.json.json {
        let mut value = serde_json::to_value(&plan).expect("a plan serialises");
        value["applied"] = json!(args.apply.apply);
        println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("block version JSON")
        );
        return OK;
    }
    println!(
        "{} {} -> {}{}",
        plan.block,
        plan.from,
        plan.to,
        if args.apply.apply {
            ""
        } else {
            " (plan; --apply writes it)"
        }
    );
    for file in &plan.files {
        println!("  {file}");
    }
    OK
}
