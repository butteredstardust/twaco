//! Workspace policy for Network connection edits.

use super::{lock_workspace, Access, Effects, Mode, Notices};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::{self, Solution};
use crate::core::entity;
use crate::core::lock;
use crate::core::network;
use crate::core::transaction::{Transaction, TransactionError};
use crate::core::workspace;
use std::collections::{BTreeSet, HashSet};
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
    let mut created = Vec::new();
    for row in &ordered {
        if known_thing(solution, &row.name) {
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
    let mut after = before.clone();
    let mut added = Vec::new();
    let mut moved = Vec::new();
    let mut noops = Vec::new();
    for row in ordered {
        let model = network::parse(&after).map_err(CommandError::Network)?;
        let existing = model
            .connections
            .iter()
            .find(|connection| connection.to == row.name);
        match existing {
            Some(connection) if connection.from == row.parent => noops.push(row.name.clone()),
            Some(connection) if !request.move_existing => {
                return Err(CommandError::Invalid(format!(
                "CSV line {}: {} is already in Network {} under {}; use --move to put it under {}",
                row.line, row.name, model.name,
                if connection.from.is_empty() { "the root" } else { &connection.from },
                if row.parent.is_empty() { "the root" } else { &row.parent }
            )))
            }
            Some(connection) => {
                let without = network::remove(&after, &model, &connection.from, &connection.to)
                    .map_err(CommandError::Network)?;
                let revised = network::parse(&without).map_err(CommandError::Network)?;
                after = network::add(
                    &without,
                    &revised,
                    &row.connection_type,
                    &row.parent,
                    &row.name,
                )
                .map_err(CommandError::Network)?;
                moved.push(row.name.clone());
            }
            None => {
                after = network::add(&after, &model, &row.connection_type, &row.parent, &row.name)
                    .map_err(CommandError::Network)?;
                added.push(row.name.clone());
            }
        }
    }
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
}
