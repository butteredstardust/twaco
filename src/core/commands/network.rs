//! Workspace policy for Network connection edits.

use super::{lock_workspace, Access, Effects, Mode, Notices};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::{self, Solution};
use crate::core::entity;
use crate::core::lock;
use crate::core::network;
use crate::core::profile;
use crate::core::server::ServerError;
use crate::core::transaction::{Transaction, TransactionError};
use crate::core::workspace;
use std::collections::{BTreeSet, HashMap, HashSet};
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
/// The file-driven form of a network edit.  Kept separate from [`Request`] because one import
/// can create several Things as well as edit one Network.
#[derive(Clone, Debug)]
pub struct ImportRequest {
    pub network: String,
    pub file: PathBuf,
    pub create_things: bool,
    pub move_existing: bool,
    pub outside: bool,
    pub mode: Mode,
    pub lock_label: &'static str,
}
#[derive(Debug)]
pub struct ImportPlan {
    pub path: PathBuf,
    pub before: Vec<u8>,
    pub after: Vec<u8>,
    pub created: Vec<entity::new::Plan>,
    pub added: Vec<String>,
    pub moved: Vec<String>,
    pub noops: Vec<String>,
}
#[derive(Debug)]
pub struct ImportOutcome {
    pub plan: ImportPlan,
    pub applied: bool,
    #[allow(dead_code, reason = "the command facade records workspace effects")]
    pub(crate) effects: Effects,
}
/// A server/repository connection comparison, grouped by the Thing whose placement differs.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Diff {
    pub server_only: Vec<network::ServerConnection>,
    pub repository_only: Vec<network::Connection>,
    pub different: Vec<(network::Connection, network::ServerConnection)>,
}
impl Diff {
    pub fn differs(&self) -> bool {
        !self.server_only.is_empty()
            || !self.repository_only.is_empty()
            || !self.different.is_empty()
    }
}
#[derive(Clone, Debug)]
pub struct PullRequest {
    pub network: String,
    pub mode: Mode,
    pub profile: String,
    pub lock_label: &'static str,
}
#[derive(Debug)]
pub struct PullPlan {
    pub path: PathBuf,
    pub before: Vec<u8>,
    pub after: Vec<u8>,
    pub server_connections: usize,
    pub outside_repository: usize,
    pub added: usize,
    pub removed: usize,
    pub changed: usize,
}
#[derive(Debug)]
pub struct PullOutcome {
    pub plan: PullPlan,
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
    #[error("{0}")]
    Profile(profile::ProfileError),
    #[error("{0}")]
    Server(ServerError),
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
            Self::Profile(error) => error.code(),
            Self::Server(error) => error.code(),
        }
    }
}

/// Compare connection placement by child. A Thing can have one parent, so equal child names
/// with a changed parent or type are one useful difference rather than two noisy additions.
pub fn diff(repository: &network::Network, server: &[network::ServerConnection]) -> Diff {
    let repo_by_to: std::collections::HashMap<_, _> = repository
        .connections
        .iter()
        .map(|row| (row.to.as_str(), row))
        .collect();
    let server_by_to: std::collections::HashMap<_, _> =
        server.iter().map(|row| (row.to.as_str(), row)).collect();
    let mut result = Diff::default();
    for row in server {
        match repo_by_to.get(row.to.as_str()) {
            None => result.server_only.push(row.clone()),
            Some(local)
                if local.from != row.from || local.connection_type != row.connection_type =>
            {
                result.different.push(((*local).clone(), row.clone()))
            }
            Some(_) => {}
        }
    }
    for row in &repository.connections {
        if !server_by_to.contains_key(row.to.as_str()) {
            result.repository_only.push(row.clone());
        }
    }
    result
}

pub fn server_diff<R: network::Remote>(
    solution: &Solution,
    name: &str,
    remote: &R,
) -> Result<Diff, CommandError> {
    let file = network_file(solution, name)?;
    let bytes = std::fs::read(&file.path)
        .map_err(|error| CommandError::Invalid(format!("{}: {error}", file.path.display())))?;
    let repository = network::parse(&bytes).map_err(CommandError::Network)?;
    Ok(diff(
        &repository,
        &remote
            .connections(name, false)
            .map_err(CommandError::Server)?,
    ))
}

/// Read the profile only for the server-backed form, keeping offline network commands offline.
pub fn server_diff_execute<R, F>(
    solution: &Solution,
    name: &str,
    profile_name: &str,
    open: F,
) -> Result<Diff, CommandError>
where
    R: network::Remote,
    F: FnOnce(profile::Profile) -> R,
{
    let profile = profile::load(&solution.root, profile_name).map_err(CommandError::Profile)?;
    server_diff(solution, name, &open(profile))
}

/// Read the hierarchy the server currently has. Templates deliberately come from the same
/// service response, not the repository's potentially stale Thing exports.
pub fn server_show<R, F>(
    solution: &Solution,
    name: &str,
    profile_name: &str,
    open: F,
) -> Result<network::Network, CommandError>
where
    R: network::Remote,
    F: FnOnce(profile::Profile) -> R,
{
    let file = network_file(solution, name)?;
    let bytes = std::fs::read(&file.path)
        .map_err(|error| CommandError::Invalid(format!("{}: {error}", file.path.display())))?;
    let local = network::parse(&bytes).map_err(CommandError::Network)?;
    let profile = profile::load(&solution.root, profile_name).map_err(CommandError::Profile)?;
    let rows = open(profile)
        .connections(name, true)
        .map_err(CommandError::Server)?;
    Ok(network::from_server(&local.name, &local.project, &rows))
}

fn pull_plan<R: network::Remote>(
    solution: &Solution,
    name: &str,
    remote: &R,
) -> Result<PullPlan, CommandError> {
    let file = network_file(solution, name)?;
    let before = std::fs::read(&file.path)
        .map_err(|error| CommandError::Invalid(format!("{}: {error}", file.path.display())))?;
    let initial = network::parse(&before).map_err(CommandError::Network)?;
    let rows = remote
        .connections(name, false)
        .map_err(CommandError::Server)?;
    // Only the connections that differ are edited, so a pull of an unchanged server writes
    // nothing and a pulled change reads as a small diff.
    let changes = diff(&initial, &rows);
    let replaced: BTreeSet<String> = changes
        .repository_only
        .iter()
        .map(|row| row.to.clone())
        .chain(changes.different.iter().map(|(row, _)| row.to.clone()))
        .collect();
    let removed =
        network::remove_children(&before, &initial, &replaced).map_err(CommandError::Network)?;
    let incoming: HashSet<&str> = changes
        .server_only
        .iter()
        .map(|row| row.to.as_str())
        .chain(changes.different.iter().map(|(_, row)| row.to.as_str()))
        .collect();
    let additions: Vec<(&str, &str, &str)> = rows
        .iter()
        .filter(|row| incoming.contains(row.to.as_str()))
        .map(|row| {
            (
                row.connection_type.as_str(),
                row.from.as_str(),
                row.to.as_str(),
            )
        })
        .collect();
    let model = network::parse(&removed).map_err(CommandError::Network)?;
    let after = network::add_all(&removed, &model, &additions).map_err(CommandError::Network)?;
    let things = repository_things(solution);
    let outside_repository = rows
        .iter()
        .filter(|row| !things.contains(row.to.as_str()))
        .count();
    Ok(PullPlan {
        path: file.path,
        before,
        after,
        server_connections: rows.len(),
        outside_repository,
        added: changes.server_only.len(),
        removed: changes.repository_only.len(),
        changed: changes.different.len(),
    })
}

/// Pull is a local transaction, but it always reads the server first. Applying reopens the
/// profile after the lock and replans, so an old plan cannot overwrite a newer local file.
pub fn pull_execute<R, F>(
    solution: &Solution,
    request: &PullRequest,
    open: F,
    notices: &mut Notices,
) -> Result<PullOutcome, CommandError>
where
    R: network::Remote,
    F: FnOnce(profile::Profile) -> R,
{
    let profile = profile::load(&solution.root, &request.profile).map_err(CommandError::Profile)?;
    if request.mode == Mode::Plan {
        return Ok(PullOutcome {
            plan: pull_plan(solution, &request.network, &open(profile))?,
            applied: false,
            effects: Effects::new(Access::Read, Access::Read),
        });
    }
    let lock = lock_workspace(solution, request.lock_label, notices).map_err(CommandError::Lock)?;
    let reloaded =
        Solution::load(&solution.root.join(config::CONFIG_FILE)).map_err(CommandError::Config)?;
    let plan = pull_plan(&reloaded, &request.network, &open(profile))?;
    let mut transaction = Transaction::new(&reloaded.root, "network pull");
    transaction
        .replace_file(&plan.path, &plan.before, plan.after.clone())
        .map_err(CommandError::Write)?;
    transaction.apply(&lock).map_err(CommandError::Write)?;
    Ok(PullOutcome {
        plan,
        applied: true,
        effects: Effects::new(Access::Write, Access::Read),
    })
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
    repository_things(solution).contains(name)
}
/// The repository's Thing names, read once for commands that look up many.
fn repository_things(solution: &Solution) -> HashSet<String> {
    workspace::entities(solution)
        .into_iter()
        .filter(|entry| entry.info.collection == "Things")
        .map(|entry| entry.info.name)
        .collect()
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

/// Plan every CSV row before returning a writeable result.  The mutable byte buffer is only a
/// succession of lexical `network::add`/`remove` edits; it is never an XML serialisation.
pub fn import_plan(
    solution: &Solution,
    request: &ImportRequest,
) -> Result<ImportPlan, CommandError> {
    let csv = std::fs::read(&request.file)
        .map_err(|error| CommandError::Invalid(format!("{}: {error}", request.file.display())))?;
    let rows = network::assets_csv(&csv).map_err(CommandError::Network)?;
    let file = network_file(solution, &request.network)?;
    let before = std::fs::read(&file.path)
        .map_err(|error| CommandError::Invalid(format!("{}: {error}", file.path.display())))?;
    let initial = network::parse(&before).map_err(CommandError::Network)?;
    let names: HashSet<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    for row in &rows {
        if !row.parent.is_empty()
            && !names.contains(row.parent.as_str())
            && !initial
                .connections
                .iter()
                .any(|connection| connection.to == row.parent)
        {
            return Err(CommandError::Invalid(format!(
                "CSV line {}: parent {} is neither in Network {} nor in the CSV file",
                row.line, row.parent, initial.name
            )));
        }
    }
    // Parent-before-child, retaining CSV order among siblings.  The preceding parent check
    // turns an incomplete sort into a useful refusal rather than a partial import.
    let mut pending: Vec<&network::AssetRow> = rows.iter().collect();
    let mut ordered = Vec::new();
    let mut available: HashSet<String> = initial
        .connections
        .iter()
        .map(|connection| connection.to.clone())
        .collect();
    while !pending.is_empty() {
        let Some(at) = pending
            .iter()
            .position(|row| row.parent.is_empty() || available.contains(&row.parent))
        else {
            let row = pending[0];
            return Err(CommandError::Invalid(format!(
                "CSV line {}: {} is in a parent cycle",
                row.line, row.name
            )));
        };
        let row = pending.remove(at);
        available.insert(row.name.clone());
        ordered.push(row);
    }
    let things = repository_things(solution);
    let mut created = Vec::new();
    for row in &ordered {
        if things.contains(&row.name) {
            continue;
        }
        if request.create_things {
            let Some(template) = &row.template else {
                return Err(CommandError::Invalid(format!(
                    "CSV line {}: {} needs template with --create-things",
                    row.line, row.name
                )));
            };
            let mut new = entity::new::NewRequest::blank(entity::new::Kind::Thing, &row.name);
            new.project = row
                .project
                .clone()
                .or_else(|| Some(initial.project.clone()));
            new.template = Some(template.clone());
            new.description = row.description.clone();
            created.push(entity::new::plan(solution, &new).map_err(|error| {
                CommandError::Invalid(format!("CSV line {}: {error}", row.line))
            })?);
        } else if !request.outside {
            return Err(CommandError::Invalid(format!("CSV line {}: {} is not a Thing in the repository; pass --create-things or --outside", row.line, row.name)));
        }
    }
    // Decide every row against the placement so far, then splice once: removals of the moved
    // Things, and the additions in row order, as row-by-row edits would leave them.
    let mut placed: HashMap<String, String> = initial
        .connections
        .iter()
        .map(|connection| (connection.to.clone(), connection.from.clone()))
        .collect();
    let mut moving = BTreeSet::new();
    let mut additions: Vec<(&str, &str, &str)> = Vec::new();
    let mut added = Vec::new();
    let mut moved = Vec::new();
    let mut noops = Vec::new();
    for row in ordered {
        match placed.get(&row.name) {
            Some(parent) if *parent == row.parent => noops.push(row.name.clone()),
            Some(parent) if !request.move_existing => {
                return Err(CommandError::Invalid(format!(
                "CSV line {}: {} is already in Network {} under {}; use --move to put it under {}",
                row.line, row.name, initial.name,
                if parent.is_empty() { "the root" } else { parent },
                if row.parent.is_empty() { "the root" } else { &row.parent }
            )))
            }
            Some(_) => {
                additions.retain(|(_, _, to)| *to != row.name);
                moving.insert(row.name.clone());
                additions.push((&row.connection_type, &row.parent, &row.name));
                moved.push(row.name.clone());
            }
            None => {
                additions.push((&row.connection_type, &row.parent, &row.name));
                added.push(row.name.clone());
            }
        }
        placed.insert(row.name.clone(), row.parent.clone());
    }
    let without =
        network::remove_children(&before, &initial, &moving).map_err(CommandError::Network)?;
    let model = network::parse(&without).map_err(CommandError::Network)?;
    let after = network::add_all(&without, &model, &additions).map_err(CommandError::Network)?;
    // A --move can put a Thing under its own descendant; whatever the rows do, the network must
    // come out with no problem it did not already have.
    let had: BTreeSet<String> = network::forest(&initial)
        .problems
        .iter()
        .map(ToString::to_string)
        .collect();
    let result = network::parse(&after).map_err(CommandError::Network)?;
    if let Some(problem) = network::forest(&result)
        .problems
        .iter()
        .find(|problem| !had.contains(&problem.to_string()))
    {
        return Err(CommandError::Invalid(format!(
            "the import would leave Network {} with a problem: {problem}",
            initial.name
        )));
    }
    Ok(ImportPlan {
        path: file.path,
        before,
        after,
        created,
        added,
        moved,
        noops,
    })
}

pub fn import_execute(
    solution: &Solution,
    request: &ImportRequest,
    notices: &mut Notices,
) -> Result<ImportOutcome, CommandError> {
    if request.mode == Mode::Plan {
        return Ok(ImportOutcome {
            plan: import_plan(solution, request)?,
            applied: false,
            effects: Effects::new(Access::Read, Access::None),
        });
    }
    let lock = lock_workspace(solution, request.lock_label, notices).map_err(CommandError::Lock)?;
    let reloaded =
        Solution::load(&solution.root.join(config::CONFIG_FILE)).map_err(CommandError::Config)?;
    let plan = import_plan(&reloaded, request)?;
    let mut transaction = Transaction::new(&reloaded.root, "network import");
    for created in &plan.created {
        transaction
            .create_file(&created.entity_path, created.entity.clone())
            .map_err(CommandError::Write)?;
        for (path, bytes) in &created.sidecars {
            transaction
                .create_file(path, bytes.clone())
                .map_err(CommandError::Write)?;
        }
    }
    transaction
        .replace_file(&plan.path, &plan.before, plan.after.clone())
        .map_err(CommandError::Write)?;
    transaction.apply(&lock).map_err(CommandError::Write)?;
    Ok(ImportOutcome {
        plan,
        applied: true,
        effects: Effects::new(Access::Write, Access::None),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    struct FakeServer(Vec<network::ServerConnection>);
    impl network::Remote for FakeServer {
        fn connections(
            &self,
            _: &str,
            _: bool,
        ) -> Result<Vec<network::ServerConnection>, crate::core::server::ServerError> {
            Ok(self.0.clone())
        }
    }

    const NETWORK: &str = "<Entities>\n    <Networks>\n        <Network name=\"Acme.Plant\" projectName=\"Acme\">\n            <Connections>\n                <Connection\n                 connectionType=\"Contains\"\n                 from=\"\"\n                 to=\"Acme.Site\"></Connection>\n                <Connection\n                 connectionType=\"Contains\"\n                 from=\"Acme.Site\"\n                 to=\"Acme.LineA\"></Connection>\n                <Connection\n                 connectionType=\"Contains\"\n                 from=\"Acme.LineA\"\n                 to=\"Acme.Press\"></Connection>\n                <Connection\n                 connectionType=\"Contains\"\n                 from=\"Acme.Site\"\n                 to=\"Acme.LineB\"></Connection>\n            </Connections>\n        </Network>\n    </Networks>\n</Entities>\n";

    fn thing(name: &str) -> String {
        format!("<Entities><Things><Thing name=\"{name}\" projectName=\"Acme\" thingTemplate=\"GenericThing\"></Thing></Things></Entities>\n")
    }

    /// A Network over four Things of the repository, plus a fifth Thing not yet in it.
    fn solution() -> (tempfile::TempDir, Solution) {
        let dir = tempfile::Builder::new()
            .prefix("twaco-network-command-")
            .tempdir()
            .unwrap();
        let root = dir.path();
        for folder in ["Networks", "Things", "ThingTemplates"] {
            std::fs::create_dir_all(root.join(folder)).unwrap();
        }
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"Acme\"\n").unwrap();
        std::fs::write(root.join("Networks/Acme.Plant.xml"), NETWORK).unwrap();
        for name in [
            "Acme.Site",
            "Acme.LineA",
            "Acme.LineB",
            "Acme.Press",
            "Acme.Saw",
        ] {
            std::fs::write(root.join(format!("Things/{name}.xml")), thing(name)).unwrap();
        }
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (dir, solution)
    }

    fn request(action: Action, thing: &str, under: Option<&str>) -> Request {
        Request {
            action,
            network: "Acme.Plant".into(),
            thing: thing.into(),
            parent: under.map(str::to_string),
            root: under.is_none(),
            connection_type: None,
            subtree: false,
            outside: false,
            mode: Mode::Plan,
            lock_label: "test network",
        }
    }

    fn network_text(root: &Path) -> String {
        std::fs::read_to_string(root.join("Networks/Acme.Plant.xml")).unwrap()
    }

    fn refused(solution: &Solution, request: &Request) -> String {
        plan(solution, request).unwrap_err().to_string()
    }

    #[test]
    fn edits_refuse_what_the_server_would() {
        let (_dir, solution) = solution();
        let check = |request: Request, words: &str| {
            let why = refused(&solution, &request);
            assert!(why.contains(words), "{why}");
        };
        check(
            request(Action::Add, "Acme.Press", Some("Acme.LineB")),
            "already in Network Acme.Plant under Acme.LineA",
        );
        check(
            request(Action::Add, "Acme.Saw", Some("Acme.Nowhere")),
            "parent Acme.Nowhere is not in Network",
        );
        check(
            request(Action::Add, "Acme.Ghost", Some("Acme.LineB")),
            "pass --outside",
        );
        check(
            request(Action::Move, "Acme.Saw", Some("Acme.LineB")),
            "is not in Network",
        );
        check(
            request(Action::Move, "Acme.Site", Some("Acme.Press")),
            "under its own descendant",
        );
        check(
            request(Action::Remove, "Acme.LineA", None),
            "pass --subtree",
        );
        // A Thing only the server holds, said so.
        let mut outside = request(Action::Add, "Acme.Ghost", Some("Acme.LineB"));
        outside.outside = true;
        assert!(plan(&solution, &outside).unwrap().changed);
    }

    #[test]
    fn a_move_keeps_the_subtree_and_a_plan_writes_nothing() {
        let (dir, solution) = solution();
        let moved = plan(
            &solution,
            &request(Action::Move, "Acme.LineA", Some("Acme.LineB")),
        )
        .unwrap();
        assert_eq!(network_text(dir.path()), NETWORK);
        let after = network::parse(&moved.after).unwrap();
        let forest = network::forest(&after);
        assert!(forest.problems.is_empty(), "{:?}", forest.problems);
        assert_eq!(forest.children["Acme.LineB"], ["Acme.LineA"]);
        assert_eq!(forest.children["Acme.LineA"], ["Acme.Press"]);
    }

    #[test]
    fn apply_writes_through_the_lock_and_a_subtree_goes_whole() {
        let (dir, solution) = solution();
        let mut remove = request(Action::Remove, "Acme.LineA", None);
        remove.subtree = true;
        remove.mode = Mode::Apply;
        let outcome = execute(&solution, &remove, &mut Notices::default()).unwrap();
        assert!(outcome.applied);
        assert_eq!(outcome.plan.removed, ["Acme.Press"]);
        let text = network_text(dir.path());
        assert!(
            !text.contains("Acme.LineA") && !text.contains("Acme.Press"),
            "{text}"
        );
        assert!(text.contains("to=\"Acme.LineB\""), "{text}");
    }

    fn import(
        solution: &Solution,
        csv: &str,
        change: impl FnOnce(&mut ImportRequest),
    ) -> Result<ImportPlan, CommandError> {
        let file = solution.root.join("assets.csv");
        std::fs::write(&file, csv).unwrap();
        let mut request = ImportRequest {
            network: "Acme.Plant".into(),
            file,
            create_things: false,
            move_existing: false,
            outside: false,
            mode: Mode::Plan,
            lock_label: "test import",
        };
        change(&mut request);
        import_plan(solution, &request)
    }

    #[test]
    fn import_orders_parents_first_and_keeps_the_file_order_otherwise() {
        let (_dir, solution) = solution();
        let plan = import(
            &solution,
            "name,parent,template\r\nAcme.Cell2,Acme.Area,GenericThing\r\n\r\nAcme.Area,Acme.Site,GenericThing\r\nAcme.Cell1,Acme.Area,GenericThing\r\nAcme.Press,Acme.LineA,\r\n",
            |request| request.create_things = true,
        )
        .unwrap();
        assert_eq!(plan.added, ["Acme.Area", "Acme.Cell2", "Acme.Cell1"]);
        assert_eq!(plan.noops, ["Acme.Press"]);
        assert_eq!(plan.created.len(), 3);
    }

    #[test]
    fn import_refuses_conflicts_and_a_move_under_its_own_descendant() {
        let (_dir, solution) = solution();
        let refused = |csv: &str, moving: bool, creating: bool| {
            import(&solution, csv, |request| {
                request.move_existing = moving;
                request.create_things = creating;
            })
            .unwrap_err()
            .to_string()
        };
        let conflict = refused("name,parent\nAcme.Press,Acme.LineB\n", false, false);
        assert!(conflict.contains("use --move"), "{conflict}");
        let moved = import(
            &solution,
            "name,parent\nAcme.Press,Acme.LineB\n",
            |request| {
                request.move_existing = true;
            },
        )
        .unwrap();
        assert_eq!(moved.moved, ["Acme.Press"]);
        let cycle = refused("name,parent\nAcme.Site,Acme.Press\n", true, false);
        assert!(
            cycle.contains("would leave Network Acme.Plant with a problem"),
            "{cycle}"
        );
        let unknown = refused("name,parent\nAcme.New,Acme.Site\n", false, false);
        assert!(
            unknown.contains("--create-things or --outside"),
            "{unknown}"
        );
        let no_template = refused("name,parent\nAcme.New,Acme.Site\n", false, true);
        assert!(no_template.contains("needs template"), "{no_template}");
    }

    #[test]
    fn an_applied_import_creates_its_things_with_the_network_and_a_refusal_writes_nothing() {
        let (dir, solution) = solution();
        let file = dir.path().join("assets.csv");
        std::fs::write(
            &file,
            "name,parent,template,description\nAcme.Area,Acme.Site,GenericThing,\"Area, north\"\n",
        )
        .unwrap();
        let request = ImportRequest {
            network: "Acme.Plant".into(),
            file: file.clone(),
            create_things: true,
            move_existing: false,
            outside: false,
            mode: Mode::Apply,
            lock_label: "test import",
        };
        let outcome = import_execute(&solution, &request, &mut Notices::default()).unwrap();
        assert!(outcome.applied);
        assert!(dir.path().join("Things/Acme.Area.xml").is_file());
        assert!(network_text(dir.path()).contains("to=\"Acme.Area\""));

        let before = network_text(dir.path());
        std::fs::write(
            &file,
            "name,parent,template\nAcme.Zone,Acme.Site,GenericThing\nAcme.Press,Acme.LineB,\n",
        )
        .unwrap();
        assert!(import_execute(&solution, &request, &mut Notices::default()).is_err());
        assert_eq!(network_text(dir.path()), before);
        assert!(!dir.path().join("Things/Acme.Zone.xml").exists());
    }

    #[test]
    fn server_diff_groups_a_reparent_as_one_changed_connection() {
        let (_dir, solution) = solution();
        let server = FakeServer(vec![
            network::ServerConnection {
                connection_type: "Contains".into(),
                from: "".into(),
                to: "Acme.Site".into(),
                template: None,
            },
            network::ServerConnection {
                connection_type: "Contains".into(),
                from: "Acme.LineB".into(),
                to: "Acme.LineA".into(),
                template: None,
            },
            network::ServerConnection {
                connection_type: "Contains".into(),
                from: "Acme.LineA".into(),
                to: "Acme.Press".into(),
                template: None,
            },
            network::ServerConnection {
                connection_type: "Contains".into(),
                from: "Acme.Site".into(),
                to: "Acme.LineB".into(),
                template: None,
            },
            network::ServerConnection {
                connection_type: "Contains".into(),
                from: "Acme.Site".into(),
                to: "Acme.ServerOnly".into(),
                template: None,
            },
        ]);
        let report = server_diff(&solution, "Acme.Plant", &server).unwrap();
        assert_eq!(report.different.len(), 1);
        assert_eq!(report.server_only[0].to, "Acme.ServerOnly");
        assert!(report.repository_only.is_empty());
    }

    #[test]
    fn pull_plan_splices_the_server_hierarchy_and_counts_outside_things() {
        let (_dir, solution) = solution();
        let server = FakeServer(vec![
            network::ServerConnection {
                connection_type: "Contains".into(),
                from: "".into(),
                to: "Acme.Site".into(),
                template: None,
            },
            network::ServerConnection {
                connection_type: "Child".into(),
                from: "Acme.Site".into(),
                to: "Acme.RemotePress".into(),
                template: None,
            },
        ]);
        let plan = pull_plan(&solution, "Acme.Plant", &server).unwrap();
        let pulled = network::parse(&plan.after).unwrap();
        assert_eq!(pulled.connections.len(), 2);
        assert_eq!(pulled.connections[1].connection_type, "Child");
        assert_eq!(plan.outside_repository, 1);
        assert_ne!(plan.before, plan.after);
    }

    fn row(connection_type: &str, from: &str, to: &str) -> network::ServerConnection {
        network::ServerConnection {
            connection_type: connection_type.into(),
            from: from.into(),
            to: to.into(),
            template: None,
        }
    }

    fn repository_rows() -> Vec<network::ServerConnection> {
        network::parse(NETWORK.as_bytes())
            .unwrap()
            .connections
            .iter()
            .map(|c| row(&c.connection_type, &c.from, &c.to))
            .collect()
    }

    #[test]
    fn pulling_an_unchanged_server_writes_nothing() {
        let (_dir, solution) = solution();
        let plan = pull_plan(&solution, "Acme.Plant", &FakeServer(repository_rows())).unwrap();
        assert_eq!(plan.before, plan.after);
        assert_eq!((plan.added, plan.removed, plan.changed), (0, 0, 0));
        assert_eq!(plan.outside_repository, 0);
    }

    #[test]
    fn pulling_a_reparent_changes_only_that_connection() {
        let (_dir, solution) = solution();
        let mut rows = repository_rows();
        rows[2].from = "Acme.LineB".into();
        let plan = pull_plan(&solution, "Acme.Plant", &FakeServer(rows)).unwrap();
        let before = String::from_utf8(plan.before.clone()).unwrap();
        let after = String::from_utf8(plan.after.clone()).unwrap();
        // The old Press connection left with its lines; the new one follows the last connection.
        let expected = before
            .replace(
                "                <Connection
                 connectionType=\"Contains\"
                 from=\"Acme.LineA\"
                 to=\"Acme.Press\"></Connection>
",
                "",
            )
            .replace(
                "to=\"Acme.LineB\"></Connection>
",
                "to=\"Acme.LineB\"></Connection>
                <Connection
                 connectionType=\"Contains\"
                 from=\"Acme.LineB\"
                 to=\"Acme.Press\"></Connection>
",
            );
        assert_eq!(after, expected);
        assert_eq!((plan.added, plan.removed, plan.changed), (0, 0, 1));
    }

    #[test]
    fn pulling_an_empty_server_empties_the_connections() {
        let (_dir, solution) = solution();
        let plan = pull_plan(&solution, "Acme.Plant", &FakeServer(Vec::new())).unwrap();
        assert!(String::from_utf8(plan.after)
            .unwrap()
            .contains("<Connections></Connections>"));
        assert_eq!(plan.removed, 4);
    }

    #[test]
    fn a_batch_add_equals_adding_one_by_one() {
        let rows = [
            ("Contains", "", "Acme.A"),
            ("Child", "Acme.A", "Acme.B"),
            ("Contains", "Acme.B", "Acme.C"),
        ];
        let empty_inline = "<Entities><Networks><Network name=\"Acme.N\" projectName=\"Acme\"></Network></Networks></Entities>";
        let empty_lines = "<Entities>
    <Networks>
        <Network name=\"Acme.N\" projectName=\"Acme\">
        </Network>
    </Networks>
</Entities>
";
        let empty_element = "<Entities>
    <Networks>
        <Network name=\"Acme.N\" projectName=\"Acme\">
            <Connections></Connections>
        </Network>
    </Networks>
</Entities>
";
        for src in [NETWORK, empty_lines, empty_element, empty_inline] {
            let mut one_by_one = src.as_bytes().to_vec();
            for (connection_type, from, to) in rows {
                let model = network::parse(&one_by_one).unwrap();
                one_by_one = network::add(&one_by_one, &model, connection_type, from, to).unwrap();
            }
            let model = network::parse(src.as_bytes()).unwrap();
            let batch = network::add_all(src.as_bytes(), &model, &rows).unwrap();
            if src == empty_inline {
                // Inline, the batch stays on the line; its connections are the same.
                let a = network::parse(&batch).unwrap().connections;
                let b = network::parse(&one_by_one).unwrap().connections;
                let key = |c: &network::Connection| {
                    (c.connection_type.clone(), c.from.clone(), c.to.clone())
                };
                assert_eq!(
                    a.iter().map(key).collect::<Vec<_>>(),
                    b.iter().map(key).collect::<Vec<_>>()
                );
            } else {
                assert_eq!(
                    String::from_utf8(batch).unwrap(),
                    String::from_utf8(one_by_one).unwrap()
                );
            }
        }
    }
}
