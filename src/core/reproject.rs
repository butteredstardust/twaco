//! `twaco reproject <Entity>... --to <Project>`: move entities to another project of the solution.
//!
//! An entity belongs to the project its `projectName` names, and is filed under that project's
//! folders. A move changes the attribute (one span, nothing re-serialised) and, when the two
//! projects keep their entities in different folders, the file's place: the same collection
//! folder, and subfolder, under the other project's root. Sidecars are filed by entity name, so
//! they stay. Every file changes in one transaction, or none does.
//!
//! What a move can break is the deploy order: an entity of a project that does not depend on the
//! new project cannot use the moved entity, and the moved entity cannot use one of a project the
//! new project does not depend on. The plan names each such reference and the `depends_on` that
//! would allow it; nothing is refused for it, since the fix may be the next move.

use super::codes::{Coded, ErrorCode};
use super::config::Solution;
use super::index::{Confidence, Index};
use super::lock::WorkspaceLock;
use super::splice::{self, Edit};
use super::transaction::{Transaction, TransactionError};
use super::{scan, sidecar, workspace};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Request {
    /// `Name` or `Collection/Name`.
    pub entities: Vec<String>,
    pub to: String,
}

/// One entity's move.
#[derive(Clone, Debug)]
pub struct Move {
    pub collection: String,
    pub name: String,
    pub from: String,
    pub old_path: PathBuf,
    pub new_path: PathBuf,
    old: Vec<u8>,
    new: Vec<u8>,
}

#[derive(Debug)]
pub struct Plan {
    pub to: String,
    pub moves: Vec<Move>,
    /// References the move leaves outside the deploy order, each with the fix.
    pub warnings: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ReprojectError {
    #[error("{0}")]
    Unknown(String),
    #[error("{0}")]
    Invalid(String),
    #[error("nothing was moved: {0}")]
    Write(TransactionError),
}

impl Coded for ReprojectError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Unknown(_) => ErrorCode::UnknownEntity,
            Self::Invalid(_) => ErrorCode::InvalidArguments,
            Self::Write(error) => error.code(),
        }
    }
}

pub(crate) fn plan(solution: &Solution, request: &Request) -> Result<Plan, ReprojectError> {
    let target = solution.project(&request.to).ok_or_else(|| {
        ReprojectError::Unknown(format!(
            "project {} is not in twaco.toml; declare it first",
            request.to
        ))
    })?;
    if request.entities.is_empty() {
        return Err(ReprojectError::Invalid(
            "name at least one entity".to_string(),
        ));
    }
    let found = workspace::discover(solution);
    let mut moves: Vec<Move> = Vec::new();
    for wanted in &request.entities {
        let (collection, name) = match wanted.split_once('/') {
            Some((collection, name)) => (Some(collection), name),
            None => (None, wanted.as_str()),
        };
        let mut matching: Vec<&workspace::EntityFile> = found
            .entities
            .iter()
            .filter(|file| {
                file.info.name == name && collection.is_none_or(|c| c == file.info.collection)
            })
            .collect();
        // Projects that share a root each find the same file: one file is one entity, taken as
        // found under its own project where it is.
        matching.sort_by_key(|file| (file.path.clone(), file.info.project != file.found_under));
        matching.dedup_by(|later, earlier| later.path == earlier.path);
        let file = match matching.as_slice() {
            [file] => *file,
            [] => {
                return Err(ReprojectError::Unknown(format!(
                    "no entity {wanted} in the solution"
                )))
            }
            _ => {
                return Err(ReprojectError::Invalid(format!(
                    "{wanted} is in more than one collection; name it as Collection/Name"
                )))
            }
        };
        if file.info.collection == "Projects" {
            return Err(ReprojectError::Invalid(format!(
                "{wanted} is a project; a project is not moved into another"
            )));
        }
        if file.info.project == request.to {
            return Err(ReprojectError::Invalid(format!(
                "{wanted} is already in project {}",
                request.to
            )));
        }
        if moves.iter().any(|planned| {
            planned.name == file.info.name && planned.collection == file.info.collection
        }) {
            continue;
        }
        // A misfiled entity, or one of a project twaco.toml does not declare, is the project
        // gate's to report and a person's to fix: a move would hide it.
        if file.info.project != file.found_under {
            return Err(ReprojectError::Invalid(format!(
                "{wanted} declares project {:?} but is filed under {}; fix that first (twaco check)",
                file.info.project, file.found_under
            )));
        }
        let from = solution.project(&file.found_under).ok_or_else(|| {
            ReprojectError::Invalid(format!("{wanted} is in no declared project"))
        })?;
        if !target.collections.is_empty() && !target.collections.contains(&file.info.collection) {
            return Err(ReprojectError::Invalid(format!(
                "project {} keeps only {}; add {} to its collections first",
                target.name,
                target.collections.join(", "),
                file.info.collection
            )));
        }
        // The same place under the other project's root: collection folder and subfolders.
        let relative = file
            .path
            .strip_prefix(solution.project_root(from))
            .map_err(|_| {
                ReprojectError::Invalid(format!(
                    "{} is not under project {}'s root",
                    file.path.display(),
                    from.name
                ))
            })?;
        let new_path = solution.project_root(target).join(relative);
        if new_path != file.path && new_path.exists() {
            return Err(ReprojectError::Invalid(format!(
                "{} already exists; nothing was moved",
                new_path.display()
            )));
        }
        let old = std::fs::read(&file.path).map_err(|error| {
            ReprojectError::Invalid(format!("{}: {error}", file.path.display()))
        })?;
        let new = with_project(&old, &request.to)
            .map_err(|why| ReprojectError::Invalid(format!("{}: {why}", file.path.display())))?;
        moves.push(Move {
            collection: file.info.collection.clone(),
            name: file.info.name.clone(),
            from: file.info.project.clone(),
            old_path: file.path.clone(),
            new_path,
            old,
            new,
        });
    }
    let warnings = order_warnings(solution, &moves, &request.to);
    Ok(Plan {
        to: request.to.clone(),
        moves,
        warnings,
    })
}

/// The document with its entity's `projectName` set to `project`, in place.
fn with_project(src: &[u8], project: &str) -> Result<Vec<u8>, String> {
    let tokens = scan::tokenize(src).map_err(|error| error.to_string())?;
    let entity = sidecar::entity_element(&tokens, src).ok_or("not an entity document")?;
    let span = scan::attribute(src, &tokens[entity], "projectName")
        .map_err(|error| error.to_string())?
        .ok_or("the entity has no projectName attribute")?;
    splice::splice(
        src,
        &[Edit::new(
            span,
            scan::escape_attribute(project)
                .map_err(|error| error.to_string())?
                .into_bytes(),
        )],
    )
    .map_err(|error| error.to_string())
}

/// Each reference the moves put outside the deploy order: a referrer whose project does not
/// reach the moved entity's new project, or a target the new project does not reach.
fn order_warnings(solution: &Solution, moves: &[Move], to: &str) -> Vec<String> {
    let index = Index::build(solution);
    let mut project_of: BTreeMap<String, String> = index
        .entities()
        .map(|(key, node)| (key.to_string(), node.project.clone()))
        .collect();
    let moved: BTreeSet<String> = moves
        .iter()
        .map(|item| format!("{}/{}", item.collection, item.name))
        .collect();
    for label in &moved {
        project_of.insert(label.clone(), to.to_string());
    }
    // A project node (its entry point, its post-import calls in twaco.toml) is its own project.
    for project in &solution.projects {
        project_of.insert(format!("project {}", project.name), project.name.clone());
    }
    let reaches = |from: &str, to: &str| -> bool {
        let mut seen = BTreeSet::new();
        let mut pending = vec![from.to_string()];
        while let Some(project) = pending.pop() {
            if project == to {
                return true;
            }
            if seen.insert(project.clone()) {
                if let Some(declared) = solution.project(&project) {
                    pending.extend(declared.depends_on.iter().cloned());
                }
            }
        }
        false
    };
    let mut warnings = BTreeSet::new();
    for (source, target, edge) in index.edges() {
        if edge.confidence() < Confidence::Resolved
            || !(moved.contains(&source) || moved.contains(&target))
        {
            continue;
        }
        let (Some(from), Some(needed)) = (project_of.get(&source), project_of.get(&target)) else {
            continue;
        };
        if !reaches(from, needed) {
            warnings.insert(format!(
                "{source} (project {from}) uses {target} (project {needed}): add \"{needed}\" to \
                 project {from}'s depends_on, or move one of them too"
            ));
        }
    }
    warnings.into_iter().collect()
}

/// The plan as JSON, the same for the command line and MCP.
pub fn plan_json(solution: &Solution, plan: &Plan, applied: bool) -> serde_json::Value {
    let relative = |path: &std::path::Path| solution.relative(path);
    serde_json::json!({
        "applied": applied,
        "to": plan.to,
        "moves": plan.moves.iter().map(|item| serde_json::json!({
            "entity": format!("{}/{}", item.collection, item.name),
            "from": item.from,
            "file": relative(&item.old_path),
            "new_file": relative(&item.new_path),
        })).collect::<Vec<_>>(),
        "deploy_order": plan.warnings,
    })
}

/// Write every move in one transaction: each file's new bytes, then its new place.
pub(crate) fn apply(
    solution: &Solution,
    plan: &Plan,
    lock: &WorkspaceLock,
) -> Result<(), ReprojectError> {
    let mut transaction = Transaction::new(&solution.root, "reproject");
    for item in &plan.moves {
        transaction
            .replace_file(&item.old_path, &item.old, item.new.clone())
            .map_err(ReprojectError::Write)?;
        if item.new_path != item.old_path {
            transaction
                .move_path(&item.old_path, &item.new_path)
                .map_err(ReprojectError::Write)?;
        }
    }
    transaction.apply(lock).map_err(ReprojectError::Write)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn write(root: &Path, relative: &str, text: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn thing(name: &str, project: &str, template: &str) -> String {
        format!(
            "<Entities>\n  <Things>\n    <Thing name=\"{name}\" projectName=\"{project}\" thingTemplate=\"{template}\">\n      <Owner name=\"Administrator\"/>\n    </Thing>\n  </Things>\n</Entities>\n"
        )
    }

    /// Project A in `a/`, project B in `b/`; A.User's template is A.Base_TT.
    fn fixture(depends: bool) -> (tempfile::TempDir, Solution) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let depends = if depends {
            "depends_on = [\"B\"]\n"
        } else {
            ""
        };
        write(
            root,
            "twaco.toml",
            &format!("[[project]]\nname = \"A\"\nroot = \"a\"\n{depends}\n[[project]]\nname = \"B\"\nroot = \"b\"\n"),
        );
        write(
            root,
            "a/ThingTemplates/A.Base_TT.xml",
            "<Entities><ThingTemplates><ThingTemplate name=\"A.Base_TT\" projectName=\"A\" baseThingTemplate=\"GenericThing\"/></ThingTemplates></Entities>\n",
        );
        write(
            root,
            "a/Things/sub/A.User.xml",
            &thing("A.User", "A", "A.Base_TT"),
        );
        write(root, "src/A.Base_TT/note.txt", "a sidecar\n");
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (dir, solution)
    }

    fn request(entities: &[&str], to: &str) -> Request {
        Request {
            entities: entities.iter().map(|name| name.to_string()).collect(),
            to: to.to_string(),
        }
    }

    #[test]
    fn a_move_changes_only_project_name_and_files_the_entity_under_the_other_project() {
        let (_dir, solution) = fixture(true);
        let planned = plan(&solution, &request(&["A.User"], "B")).unwrap();
        assert_eq!(planned.moves.len(), 1);
        assert_eq!(
            planned.moves[0].new_path,
            solution.root.join("b/Things/sub/A.User.xml")
        );
        // A.User still uses A.Base_TT, which stays in A, and B does not depend on A.
        assert_eq!(
            planned.warnings,
            ["Things/A.User (project B) uses ThingTemplates/A.Base_TT (project A): add \"A\" to project B's depends_on, or move one of them too"]
        );
        let lock = crate::core::lock::acquire(&solution.root, "test", &[]).unwrap();
        apply(&solution, &planned, &lock).unwrap();
        assert!(!solution.root.join("a/Things/sub/A.User.xml").exists());
        assert_eq!(
            std::fs::read_to_string(solution.root.join("b/Things/sub/A.User.xml")).unwrap(),
            thing("A.User", "B", "A.Base_TT"),
            "every other byte as it was"
        );
        let found = workspace::discover(&solution);
        let moved = found
            .entities
            .iter()
            .find(|file| file.info.name == "A.User")
            .unwrap();
        assert_eq!(
            (moved.info.project.as_str(), moved.found_under.as_str()),
            ("B", "B")
        );
    }

    #[test]
    fn a_reference_the_move_puts_outside_the_deploy_order_is_named_with_its_fix() {
        // A.User (in A) uses A.Base_TT; moving the template to B needs A to depend on B.
        let (_dir, solution) = fixture(false);
        let planned = plan(&solution, &request(&["ThingTemplates/A.Base_TT"], "B")).unwrap();
        assert_eq!(
            planned.warnings,
            ["Things/A.User (project A) uses ThingTemplates/A.Base_TT (project B): add \"B\" to \
              project A's depends_on, or move one of them too"]
        );
        let (_dir, solution) = fixture(true);
        let planned = plan(&solution, &request(&["A.Base_TT"], "B")).unwrap();
        assert!(planned.warnings.is_empty(), "{:?}", planned.warnings);
        // Moving both together leaves nothing outside the order.
        let (_dir, solution) = fixture(false);
        let planned = plan(&solution, &request(&["A.Base_TT", "A.User"], "B")).unwrap();
        assert!(planned.warnings.is_empty(), "{:?}", planned.warnings);
    }

    #[test]
    fn what_cannot_be_moved_is_refused_before_anything_is_written() {
        let (_dir, solution) = fixture(true);
        let refused = |entities: &[&str], to: &str| {
            plan(&solution, &request(entities, to))
                .unwrap_err()
                .to_string()
        };
        assert!(refused(&["A.User"], "C").contains("project C is not in twaco.toml"));
        assert!(refused(&["A.Nope"], "B").contains("no entity A.Nope"));
        assert!(refused(&["A.User"], "A").contains("already in project A"));
        write(&solution.root, "b/Things/sub/A.User.xml", "taken");
        assert!(refused(&["A.User"], "B").contains("already exists"));
        assert_eq!(
            std::fs::read_to_string(solution.root.join("a/Things/sub/A.User.xml")).unwrap(),
            thing("A.User", "A", "A.Base_TT")
        );
    }

    #[test]
    fn a_misfiled_entity_and_a_collection_the_target_does_not_keep_are_refused() {
        let (_dir, solution) = fixture(true);
        write(
            &solution.root,
            "a/Things/A.Stray.xml",
            &thing("A.Stray", "Gone", "GenericThing"),
        );
        let error = plan(&solution, &request(&["A.Stray"], "B"))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("declares project \"Gone\" but is filed under A"),
            "{error}"
        );
        std::fs::write(
            solution.root.join("twaco.toml"),
            "[[project]]\nname = \"A\"\nroot = \"a\"\n\n[[project]]\nname = \"B\"\nroot = \"b\"\ncollections = [\"Things\"]\n",
        )
        .unwrap();
        let solution = Solution::load(&solution.root.join("twaco.toml")).unwrap();
        let error = plan(&solution, &request(&["A.Base_TT"], "B"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("project B keeps only Things"), "{error}");
    }

    #[test]
    fn an_entry_point_moved_out_of_its_project_is_named() {
        let (_dir, solution) = fixture(true);
        std::fs::write(
            solution.root.join("twaco.toml"),
            "[[project]]\nname = \"A\"\nroot = \"a\"\n\n[project.deploy]\nentry_point_thing = \"A.User\"\n\n[[project]]\nname = \"B\"\nroot = \"b\"\ndepends_on = [\"A\"]\n",
        )
        .unwrap();
        let solution = Solution::load(&solution.root.join("twaco.toml")).unwrap();
        let planned = plan(&solution, &request(&["A.User"], "B")).unwrap();
        assert!(
            planned
                .warnings
                .iter()
                .any(|warning| warning.starts_with("project A (project A) uses Things/A.User")),
            "{:?}",
            planned.warnings
        );
    }

    #[test]
    fn projects_sharing_a_root_change_only_the_attribute() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "twaco.toml",
            "[[project]]\nname = \"A\"\n\n[[project]]\nname = \"B\"\n",
        );
        write(root, "Things/X.xml", &thing("X", "A", "GenericThing"));
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let planned = plan(&solution, &request(&["X"], "B")).unwrap();
        assert_eq!(planned.moves[0].new_path, planned.moves[0].old_path);
        let lock = crate::core::lock::acquire(&solution.root, "test", &[]).unwrap();
        apply(&solution, &planned, &lock).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("Things/X.xml")).unwrap(),
            thing("X", "B", "GenericThing")
        );
    }
}
