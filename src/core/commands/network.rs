//! Workspace policy for Network connection edits.

use super::{lock_workspace, Access, Effects, Mode, Notices};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::{self, Solution};
use crate::core::lock;
use crate::core::network;
use crate::core::transaction::{Transaction, TransactionError};
use crate::core::workspace;
use std::collections::BTreeSet;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub enum Action {
    Add,
    Move,
    Remove,
}
#[derive(Clone, Debug)]
pub struct Request {
    pub action: Action,
    pub network: String,
    pub thing: String,
    pub parent: Option<String>,
    pub root: bool,
    pub connection_type: Option<String>,
    pub subtree: bool,
    pub outside: bool,
    pub mode: Mode,
    pub lock_label: &'static str,
}
#[derive(Debug)]
pub struct Plan {
    pub path: PathBuf,
    pub before: Vec<u8>,
    pub after: Vec<u8>,
    pub changed: bool,
    pub removed: Vec<String>,
}
#[derive(Debug)]
pub struct Outcome {
    pub plan: Plan,
    pub applied: bool,
    #[allow(dead_code, reason = "the command facade records workspace effects")]
    pub(crate) effects: Effects,
}
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Workspace(workspace::WorkspaceError),
    #[error("{0}")]
    Network(network::NetworkError),
    #[error("{0}")]
    Lock(lock::LockError),
    #[error("{0}")]
    Config(config::ConfigError),
    #[error("{0}")]
    Write(TransactionError),
}
impl Coded for CommandError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Invalid(_) => ErrorCode::InvalidArguments,
            Self::Workspace(error) => error.code(),
            Self::Network(_) => ErrorCode::InvalidData,
            Self::Lock(error) => error.code(),
            Self::Config(error) => error.code(),
            Self::Write(error) => error.code(),
        }
    }
}

fn network_file(solution: &Solution, name: &str) -> Result<workspace::EntityFile, CommandError> {
    let entities: Vec<_> = workspace::entities(solution)
        .into_iter()
        .filter(|entry| entry.info.collection == "Networks")
        .collect();
    Ok(workspace::resolve(&entities, name)
        .map_err(CommandError::Workspace)?
        .clone())
}
fn known_thing(solution: &Solution, name: &str) -> bool {
    workspace::entities(solution)
        .into_iter()
        .any(|entry| entry.info.collection == "Things" && entry.info.name == name)
}
fn descendants(forest: &network::Forest, root: &str, out: &mut BTreeSet<String>) {
    if !out.insert(root.into()) {
        return;
    }
    if let Some(children) = forest.children.get(root) {
        for child in children {
            descendants(forest, child, out);
        }
    }
}

pub fn plan(solution: &Solution, request: &Request) -> Result<Plan, CommandError> {
    if !matches!(request.action, Action::Remove) && request.root == request.parent.is_some() {
        return Err(CommandError::Invalid(
            "give --under <Thing> or --as-root".into(),
        ));
    }
    let file = network_file(solution, &request.network)?;
    let before = std::fs::read(&file.path)
        .map_err(|error| CommandError::Invalid(format!("{}: {error}", file.path.display())))?;
    let model = network::parse(&before).map_err(CommandError::Network)?;
    let forest = network::forest(&model);
    let parent = request.parent.clone().unwrap_or_default();
    let existing = model
        .connections
        .iter()
        .find(|connection| connection.to == request.thing);
    let (after, removed) = match request.action {
        Action::Add => {
            if let Some(connection) = existing {
                return Err(CommandError::Invalid(format!(
                    "{} is already in Network {} under {}; use network move",
                    request.thing,
                    model.name,
                    if connection.from.is_empty() {
                        "the root"
                    } else {
                        &connection.from
                    }
                )));
            }
            if !request.root
                && !model
                    .connections
                    .iter()
                    .any(|connection| connection.to == parent)
            {
                return Err(CommandError::Invalid(format!(
                    "parent {parent} is not in Network {}",
                    model.name
                )));
            }
            if !known_thing(solution, &request.thing) && !request.outside {
                return Err(CommandError::Invalid(format!("{} is not a Thing in the repository; pass --outside when it exists only on the server", request.thing)));
            }
            (
                network::add(
                    &before,
                    &model,
                    request.connection_type.as_deref().unwrap_or("Contains"),
                    &parent,
                    &request.thing,
                )
                .map_err(CommandError::Network)?,
                Vec::new(),
            )
        }
        Action::Move => {
            let Some(old) = existing else {
                return Err(CommandError::Invalid(format!(
                    "{} is not in Network {}",
                    request.thing, model.name
                )));
            };
            if old.from == parent {
                (before.clone(), Vec::new())
            } else {
                let mut below = BTreeSet::new();
                descendants(&forest, &request.thing, &mut below);
                if below.contains(&parent) {
                    return Err(CommandError::Invalid(format!(
                        "cannot move {} under its own descendant {parent}",
                        request.thing
                    )));
                }
                if !request.root
                    && !model
                        .connections
                        .iter()
                        .any(|connection| connection.to == parent)
                {
                    return Err(CommandError::Invalid(format!(
                        "parent {parent} is not in Network {}",
                        model.name
                    )));
                }
                let without = network::remove(&before, &model, &old.from, &old.to)
                    .map_err(CommandError::Network)?;
                let revised = network::parse(&without).map_err(CommandError::Network)?;
                (
                    network::add(
                        &without,
                        &revised,
                        request
                            .connection_type
                            .as_deref()
                            .unwrap_or(&old.connection_type),
                        &parent,
                        &request.thing,
                    )
                    .map_err(CommandError::Network)?,
                    Vec::new(),
                )
            }
        }
        Action::Remove => {
            let Some(old) = existing else {
                return Err(CommandError::Invalid(format!(
                    "{} is not in Network {}",
                    request.thing, model.name
                )));
            };
            let mut names = BTreeSet::new();
            descendants(&forest, &request.thing, &mut names);
            if names.len() > 1 && !request.subtree {
                return Err(CommandError::Invalid(format!("{} has children in Network {}; pass --subtree to remove its {} descendants too", request.thing, model.name, names.len() - 1)));
            }
            if !request.subtree {
                names.clear();
                names.insert(request.thing.clone());
            }
            (
                network::remove_children(&before, &model, &names).map_err(CommandError::Network)?,
                names.into_iter().filter(|name| name != &old.to).collect(),
            )
        }
    };
    Ok(Plan {
        path: file.path,
        changed: before != after,
        before,
        after,
        removed,
    })
}
pub fn execute(
    solution: &Solution,
    request: &Request,
    notices: &mut Notices,
) -> Result<Outcome, CommandError> {
    if request.mode == Mode::Plan {
        return Ok(Outcome {
            plan: plan(solution, request)?,
            applied: false,
            effects: Effects::new(Access::Read, Access::None),
        });
    }
    let lock = lock_workspace(solution, request.lock_label, notices).map_err(CommandError::Lock)?;
    let reloaded =
        Solution::load(&solution.root.join(config::CONFIG_FILE)).map_err(CommandError::Config)?;
    let plan = plan(&reloaded, request)?;
    let mut transaction = Transaction::new(&reloaded.root, "network edit");
    transaction
        .replace_file(&plan.path, &plan.before, plan.after.clone())
        .map_err(CommandError::Write)?;
    transaction.apply(&lock).map_err(CommandError::Write)?;
    Ok(Outcome {
        plan,
        applied: true,
        effects: Effects::new(Access::Write, Access::None),
    })
}
