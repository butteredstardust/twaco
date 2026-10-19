//! Read and edit the repository's Network entity files.

use super::flags::{Apply, Json, Profile};
use super::style;
use crate::{print_notices, DRIFT, FAILED, OK};
use twaco::core::commands::{self, Mode};
use twaco::core::config::Solution;
use twaco::core::server;
use twaco::core::{network, workspace};

#[derive(Debug, clap::Subcommand)]
pub(crate) enum Network {
    List(List),
    Show(Show),
    Diff(Diff),
    Pull(Pull),
    Add(Add),
    Move(Move),
    Remove(Remove),
    Import(Import),
}
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct List {
    #[command(flatten)]
    json: Json,
}
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Show {
    #[command(flatten)]
    json: Json,
    #[arg(long = "depth", value_name = "n")]
    depth: Option<String>,
    #[arg(long)]
    server: bool,
    #[command(flatten)]
    profile: Profile,
    #[arg(id = "operands", value_name = "operand")]
    operands: Vec<String>,
}
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Diff {
    #[command(flatten)]
    json: Json,
    #[command(flatten)]
    profile: Profile,
    #[arg(id = "operands", value_name = "operand")]
    operands: Vec<String>,
}
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Pull {
    #[command(flatten)]
    apply: Apply,
    #[command(flatten)]
    json: Json,
    #[command(flatten)]
    profile: Profile,
    #[arg(id = "operands", value_name = "operand")]
    operands: Vec<String>,
}
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Add {
    #[command(flatten)]
    apply: Apply,
    #[command(flatten)]
    json: Json,
    #[arg(long = "under", value_name = "Thing")]
    under: Option<String>,
    #[arg(long = "as-root")]
    as_root: bool,
    #[arg(long = "type", value_name = "value")]
    connection_type: Option<String>,
    #[arg(long = "outside")]
    outside: bool,
    #[arg(id = "operands", value_name = "operand")]
    operands: Vec<String>,
}
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Move {
    #[command(flatten)]
    apply: Apply,
    #[command(flatten)]
    json: Json,
    #[arg(long = "under", value_name = "Thing")]
    under: Option<String>,
    #[arg(long = "as-root")]
    as_root: bool,
    #[arg(long = "type", value_name = "value")]
    connection_type: Option<String>,
    #[arg(id = "operands", value_name = "operand")]
    operands: Vec<String>,
}
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Remove {
    #[command(flatten)]
    apply: Apply,
    #[command(flatten)]
    json: Json,
    #[arg(long)]
    subtree: bool,
    #[arg(id = "operands", value_name = "operand")]
    operands: Vec<String>,
}
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Import {
    #[command(flatten)]
    apply: Apply,
    #[command(flatten)]
    json: Json,
    #[arg(long = "file", value_name = "path")]
    file: Option<std::path::PathBuf>,
    #[arg(long = "create-things")]
    create_things: bool,
    #[arg(long = "move")]
    move_existing: bool,
    #[arg(long)]
    outside: bool,
    #[arg(id = "operands", value_name = "operand")]
    operands: Vec<String>,
}

fn resolve(solution: &Solution, name: &str) -> Result<workspace::EntityFile, String> {
    let found: Vec<_> = workspace::entities(solution)
        .into_iter()
        .filter(|entry| entry.info.collection == "Networks")
        .collect();
    workspace::resolve(&found, name)
        .cloned()
        .map_err(|error| error.to_string())
}
fn model(solution: &Solution, name: &str) -> Result<network::Network, String> {
    let file = resolve(solution, name)?;
    let bytes =
        std::fs::read(&file.path).map_err(|error| format!("{}: {error}", file.path.display()))?;
    network::parse(&bytes).map_err(|error| error.to_string())
}

pub(crate) fn network_cmd(solution: &Solution, command: &Network) -> u8 {
    match command {
        Network::List(list) => {
            let mut rows = Vec::new();
            for entry in workspace::entities(solution)
                .into_iter()
                .filter(|entry| entry.info.collection == "Networks")
            {
                match std::fs::read(&entry.path)
                    .ok()
                    .and_then(|bytes| network::parse(&bytes).ok())
                {
                    Some(model) => {
                        let forest = network::forest(&model);
                        let depth = forest.depth.values().copied().max().unwrap_or(0);
                        rows.push(serde_json::json!({"name": model.name, "project": model.project, "connections": model.connections.len(), "roots": forest.roots.len(), "depth": depth}));
                    }
                    None => eprintln!(
                        "{} could not read {}",
                        style::prefix(),
                        entry.path.display()
                    ),
                }
            }
            if list.json.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&rows).expect("network list JSON")
                );
            } else {
                for row in &rows {
                    println!(
                        "{}  {} connections, {} roots, depth {}",
                        row["name"].as_str().unwrap_or_default(),
                        row["connections"],
                        row["roots"],
                        row["depth"]
                    );
                }
            }
            OK
        }
        Network::Show(show) => {
            let (name, subtree) = match show.operands.as_slice() {
                [name] => (name, None),
                [name, top] => (name, Some(top.as_str())),
                _ => {
                    eprintln!(
                        "{} network show needs <Network> and, for one subtree, its top <Thing>",
                        style::prefix()
                    );
                    return FAILED;
                }
            };
            let network = match if show.server {
                commands::network::server_show(
                    solution,
                    name,
                    show.profile.name(),
                    server::Client::new,
                )
            } else {
                model(solution, name).map_err(commands::network::CommandError::Invalid)
            } {
                Ok(network) => network,
                Err(error) => {
                    eprintln!("{} {error}", style::prefix());
                    return FAILED;
                }
            };
            let depth = match show.depth.as_deref().map(str::parse::<usize>).transpose() {
                Ok(depth) => depth,
                Err(_) => {
                    eprintln!("{} --depth must be a non-negative integer", style::prefix());
                    return FAILED;
                }
            };
            let report = match network::show(solution, &network, subtree, depth) {
                Ok(report) => report,
                Err(error) => {
                    eprintln!("{} {error}", style::prefix());
                    return FAILED;
                }
            };
            if show.json.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&network::show_json(&network, &report))
                        .expect("network show JSON")
                );
            } else {
                for (node, level) in &report.rows {
                    println!(
                        "{}{}  [{}{}]",
                        "  ".repeat(*level),
                        node.name,
                        node.template.as_deref().unwrap_or("outside the repository"),
                        if node.connection_type == "Contains" {
                            String::new()
                        } else {
                            format!(", {}", node.connection_type)
                        }
                    );
                }
                println!(
                    "{} nodes, {} roots, depth {}",
                    report.nodes, report.roots, report.depth
                );
                for (template, count) in &report.templates {
                    println!(
                        "  {count:>6}  {}",
                        template.as_deref().unwrap_or("outside the repository")
                    );
                }
                for problem in &report.problems {
                    println!("{} {problem}", style::warn("problem:"));
                }
            }
            if report.problems.is_empty() {
                OK
            } else {
                DRIFT
            }
        }
        Network::Diff(diff) => diff_cmd(solution, diff),
        Network::Pull(pull) => pull_cmd(solution, pull),
        Network::Add(add) => edit_cmd(
            solution,
            &add.operands,
            Placement {
                parent: add.under.clone(),
                as_root: add.as_root,
                connection_type: add.connection_type.clone(),
                outside: add.outside,
            },
            add.apply.apply,
            add.json.json,
            commands::network::Action::Add,
        ),
        Network::Move(moved) => edit_cmd(
            solution,
            &moved.operands,
            Placement {
                parent: moved.under.clone(),
                as_root: moved.as_root,
                connection_type: moved.connection_type.clone(),
                outside: false,
            },
            moved.apply.apply,
            moved.json.json,
            commands::network::Action::Move,
        ),
        Network::Remove(remove) => {
            let [network, thing] = remove.operands.as_slice() else {
                eprintln!("{} network remove needs <Network> <Thing>", style::prefix());
                return FAILED;
            };
            execute(
                solution,
                commands::network::Request {
                    action: commands::network::Action::Remove,
                    network: network.clone(),
                    thing: thing.clone(),
                    parent: None,
                    root: false,
                    connection_type: None,
                    subtree: remove.subtree,
                    outside: false,
                    mode: if remove.apply.apply {
                        Mode::Apply
                    } else {
                        Mode::Plan
                    },
                    lock_label: "network remove",
                },
                remove.json.json,
            )
        }
        Network::Import(import) => import_cmd(solution, import),
    }
}

fn diff_cmd(solution: &Solution, args: &Diff) -> u8 {
    let [name] = args.operands.as_slice() else {
        eprintln!("{} network diff needs <Network>", style::prefix());
        return FAILED;
    };
    let report = match commands::network::server_diff_execute(
        solution,
        name,
        args.profile.name(),
        server::Client::new,
    ) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    if args.json.json {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({"server_only": report.server_only.iter().map(connection_json).collect::<Vec<_>>(), "repository_only": report.repository_only.iter().map(connection_json).collect::<Vec<_>>(), "different": report.different.iter().map(|(repository, server)| serde_json::json!({"repository": connection_json(repository), "server": connection_json(server)})).collect::<Vec<_>>() })).expect("network diff JSON"));
    } else if report.differs() {
        println!(
            "Network {name} differs: {} server-only, {} repository-only, {} changed",
            report.server_only.len(),
            report.repository_only.len(),
            report.different.len()
        );
    } else {
        println!("Network {name} matches the server");
    }
    if report.differs() {
        DRIFT
    } else {
        OK
    }
}

fn connection_json<T: ConnectionView>(connection: &T) -> serde_json::Value {
    serde_json::json!({"type": connection.connection_type(), "from": connection.from(), "to": connection.to()})
}
trait ConnectionView {
    fn connection_type(&self) -> &str;
    fn from(&self) -> &str;
    fn to(&self) -> &str;
}
impl ConnectionView for network::Connection {
    fn connection_type(&self) -> &str {
        &self.connection_type
    }
    fn from(&self) -> &str {
        &self.from
    }
    fn to(&self) -> &str {
        &self.to
    }
}
impl ConnectionView for network::ServerConnection {
    fn connection_type(&self) -> &str {
        &self.connection_type
    }
    fn from(&self) -> &str {
        &self.from
    }
    fn to(&self) -> &str {
        &self.to
    }
}

fn pull_cmd(solution: &Solution, args: &Pull) -> u8 {
    let [name] = args.operands.as_slice() else {
        eprintln!("{} network pull needs <Network>", style::prefix());
        return FAILED;
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::network::pull_execute(
        solution,
        &commands::network::PullRequest {
            network: name.clone(),
            mode: if args.apply.apply {
                Mode::Apply
            } else {
                Mode::Plan
            },
            profile: args.profile.name().into(),
            lock_label: "network pull",
        },
        server::Client::new,
        &mut notices,
    ) {
        Ok(value) => value,
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    let plan = &outcome.plan;
    if args.json.json {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({"applied": outcome.applied, "file": solution.relative(&plan.path), "connections": plan.server_connections, "connections_added": plan.added, "connections_removed": plan.removed, "connections_moved": plan.changed, "outside_repository": plan.outside_repository})).expect("network pull JSON"));
    } else if plan.before == plan.after {
        println!(
            "{name} already matches the server ({} connections)",
            plan.server_connections
        );
    } else {
        println!(
            "{} {name} into {}: {} added, {} removed, {} moved; {} connections, {} Things outside the repository",
            if outcome.applied { "pulled" } else { "would pull" },
            solution.relative(&plan.path),
            plan.added,
            plan.removed,
            plan.changed,
            plan.server_connections,
            plan.outside_repository
        );
        if !outcome.applied {
            println!("run again with --apply to write it");
        }
    }
    OK
}

fn import_cmd(solution: &Solution, import: &Import) -> u8 {
    let [network] = import.operands.as_slice() else {
        eprintln!("{} network import needs <Network>", style::prefix());
        return FAILED;
    };
    let Some(file) = &import.file else {
        eprintln!("{} network import needs --file <path>", style::prefix());
        return FAILED;
    };
    let request = commands::network::ImportRequest {
        network: network.clone(),
        file: file.clone(),
        create_things: import.create_things,
        move_existing: import.move_existing,
        outside: import.outside,
        mode: if import.apply.apply {
            Mode::Apply
        } else {
            Mode::Plan
        },
        lock_label: "network import",
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::network::import_execute(solution, &request, &mut notices) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    let created: Vec<_> = outcome.plan.created.iter().map(|plan| serde_json::json!({"name": plan.request.name, "template": plan.request.template, "project": plan.project})).collect();
    if import.json.json {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({
            "applied": outcome.applied, "file": solution.relative(&outcome.plan.path), "created": created,
            "added": outcome.plan.added, "moved": outcome.plan.moved, "noops": outcome.plan.noops,
        })).expect("network import JSON"));
    } else if outcome.applied {
        println!(
            "imported {}: {} Things created, {} connections added, {} moved",
            network,
            outcome.plan.created.len(),
            outcome.plan.added.len(),
            outcome.plan.moved.len()
        );
    } else {
        println!("would import {}: {} Things created, {} connections added, {} moved; run again with --apply to write it", network, outcome.plan.created.len(), outcome.plan.added.len(), outcome.plan.moved.len());
    }
    OK
}
/// Where `add` and `move` put a Thing.
struct Placement {
    parent: Option<String>,
    as_root: bool,
    connection_type: Option<String>,
    outside: bool,
}

fn edit_cmd(
    solution: &Solution,
    operands: &[String],
    placement: Placement,
    apply: bool,
    json: bool,
    action: commands::network::Action,
) -> u8 {
    let [network, thing] = operands else {
        eprintln!(
            "{} network add and move need <Network> <Thing>",
            style::prefix()
        );
        return FAILED;
    };
    execute(
        solution,
        commands::network::Request {
            action,
            network: network.clone(),
            thing: thing.clone(),
            parent: placement.parent,
            root: placement.as_root,
            connection_type: placement.connection_type,
            subtree: false,
            outside: placement.outside,
            mode: if apply { Mode::Apply } else { Mode::Plan },
            lock_label: "network edit",
        },
        json,
    )
}

/// The change in words: "add Acme.Line under Acme.Site".
fn describe(request: &commands::network::Request, removed: &[String]) -> String {
    let place = match &request.parent {
        Some(parent) if !request.root => format!("under {parent}"),
        _ => "as a root".to_string(),
    };
    match request.action {
        commands::network::Action::Add => format!("add {} {place}", request.thing),
        commands::network::Action::Move => format!("move {} {place}", request.thing),
        commands::network::Action::Remove if removed.is_empty() => {
            format!("remove {}", request.thing)
        }
        commands::network::Action::Remove => format!(
            "remove {} and its {} descendant(s): {}",
            request.thing,
            removed.len(),
            removed.join(", ")
        ),
    }
}
fn execute(solution: &Solution, request: commands::network::Request, json: bool) -> u8 {
    let mut notices = commands::Notices::default();
    let outcome = match commands::network::execute(solution, &request, &mut notices) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    if json {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({"applied":outcome.applied,"changed":outcome.plan.changed,"file":solution.relative(&outcome.plan.path),"removed":outcome.plan.removed})).expect("network edit JSON"));
    } else {
        let change = describe(&request, &outcome.plan.removed);
        let file = solution.relative(&outcome.plan.path);
        if !outcome.plan.changed {
            println!("nothing to change: {change} is already so");
        } else if outcome.applied {
            println!("{change}: updated {file}");
        } else {
            println!("would {change} in {file}; run again with --apply to write it");
        }
    }
    OK
}
