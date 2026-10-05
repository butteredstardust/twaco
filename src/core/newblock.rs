//! Create a new building block in the repository.
//!
//! A building block is a ThingWorx project with a fixed skeleton that the PTC Solution Framework
//! builds on a server with its `AddNewComponent` service: an entry point (a ThingTemplate and a
//! Thing, which names the block and its manager), a management shape and a manager template (and
//! a manager Thing unless the block is abstract), and an organization with a default and an admin
//! group whose members may see everything in the block. This writes those entities as files, in the
//! repository's own layout, and adds the project to `twaco.toml`, so a block starts life in version
//! control and reaches a server through `twaco deploy`.
//!
//! The entity text comes from templates cut from what the framework itself produced on a live
//! server (Standard, Abstract and Implementation), with the names, the description, the display name
//! and the project's dependencies left as tokens; the tests hold the output against those
//! server exports. Plan by default; nothing existing is touched except `twaco.toml`, to which the
//! new project is appended.
//!
//! Not created: the component permission helper (it generates its permission matrices on the
//! server from the block's entities, through the framework's `GetComponentPermissionsHelper`), the
//! `UI` and `Test` types (a main mashup and a navigation entry; a helper manager), and the copy of
//! a parent's manager configuration that the framework can make.

use super::config::{Solution, CONFIG_FILE};
use super::refs;
use super::workspace;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

const PROJECT: &str = include_str!("newblock/templates/Project.xml");
const DEFAULT_GROUP: &str = include_str!("newblock/templates/Group.Default_UG.xml");
const ADMIN_GROUP: &str = include_str!("newblock/templates/Group.Admin_UG.xml");
const ORGANIZATION: &str = include_str!("newblock/templates/Organization.Default_OR.xml");
const ENTRY_POINT_TEMPLATE: &str =
    include_str!("newblock/templates/ThingTemplate.EntryPoint_TT.xml");
const MANAGER_TEMPLATE: &str = include_str!("newblock/templates/ThingTemplate.Manager_TT.xml");
const MANAGEMENT_SHAPE: &str = include_str!("newblock/templates/ThingShape.Management_TS.xml");
const MODEL_LOGIC_SHAPE: &str = include_str!("newblock/templates/ThingShape.ModelLogic_TS.xml");
const ENTRY_POINT: &str = include_str!("newblock/templates/Thing.EntryPoint.xml");
const MANAGER: &str = include_str!("newblock/templates/Thing.Manager.xml");
const MAX_FILE_NAME_CHARS: usize = 200;

/// What the Manager template lists as its implemented shape; removed when there is no management shape.
const MANAGER_SHAPE_BLOCK: &str = "            <ImplementedShapes>\n                <ImplementedShape\n                 name=\"@@NAME@@.Management_TS\"\n                 type=\"ThingShape\"></ImplementedShape>\n            </ImplementedShapes>";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BlockType {
    /// A block with its own manager, built on `PTC.Base`.
    Standard,
    /// A block that other blocks implement: no manager Thing of its own.
    Abstract,
    /// A block that implements an abstract one, inheriting its templates.
    Implementation,
}

impl BlockType {
    pub fn word(self) -> &'static str {
        match self {
            BlockType::Standard => "standard",
            BlockType::Abstract => "abstract",
            BlockType::Implementation => "implementation",
        }
    }

    pub fn from_word(word: &str) -> Option<BlockType> {
        match word.to_ascii_lowercase().as_str() {
            "standard" => Some(BlockType::Standard),
            "abstract" => Some(BlockType::Abstract),
            "implementation" => Some(BlockType::Implementation),
            _ => None,
        }
    }

    /// The value the framework stores in the entry point's `componentType`.
    fn component_type(self) -> &'static str {
        match self {
            BlockType::Standard => "Standard",
            BlockType::Abstract => "Abstract",
            BlockType::Implementation => "Implementation",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Request {
    pub name: String,
    pub kind: BlockType,
    pub display_name: Option<String>,
    pub description: String,
    /// The abstract block an implementation implements.
    pub parent: Option<String>,
    /// Also create a `ModelLogic_TS` shape.
    pub model_logic: bool,
    /// Create the `Management_TS` shape (an implementation may leave it out; the others always have it).
    pub management_shape: bool,
    /// Where the project's folders go, relative to the solution; default: the block's name.
    pub root: Option<String>,
    /// The `PTC.Base` extension the project depends on (`PTC.Base:10.1.0`); default: what another project declares.
    pub base_extension: Option<String>,
}

#[derive(Debug)]
pub enum NewBlockError {
    Invalid(String),
    Exists(Vec<String>),
    Io { path: PathBuf, why: String },
}

impl fmt::Display for NewBlockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NewBlockError::Invalid(why) => f.write_str(why),
            NewBlockError::Exists(what) => write!(f, "already exists: {}", what.join(", ")),
            NewBlockError::Io { path, why } => write!(f, "{}: {why}", path.display()),
        }
    }
}

impl std::error::Error for NewBlockError {}

/// One file the plan creates.
#[derive(Debug, Clone)]
pub struct NewFile {
    pub path: PathBuf,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub request: Request,
    pub root: String,
    pub files: Vec<NewFile>,
    /// The new `twaco.toml` text, and what is appended to it.
    config_before: String,
    config_after: String,
    pub config_addition: String,
    pub notes: Vec<String>,
}

fn attribute_escaped(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Replace every `@@TOKEN@@` in one pass, so a value is never searched for tokens itself.
fn fill(template: &str, values: &BTreeMap<&str, String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("@@") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("@@") {
            Some(end) if values.contains_key(&after[..end]) => {
                out.push_str(&values[&after[..end]]);
                rest = &after[end + 2..];
            }
            _ => {
                out.push_str("@@");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn normalized_root(mut root: &str) -> &str {
    while let Some(rest) = root.strip_prefix("./") {
        root = rest;
    }
    root = root.trim_end_matches('/');
    if root == "." {
        ""
    } else {
        root
    }
}

fn roots_equal(left: &str, right: &str) -> bool {
    let left = normalized_root(left);
    let right = normalized_root(right);
    #[cfg(windows)]
    {
        left.to_lowercase() == right.to_lowercase()
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

fn is_link_or_junction(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn refuse_linked_path(root: &Path, target: &Path) -> Result<(), NewBlockError> {
    let relative = target.strip_prefix(root).map_err(|_| {
        NewBlockError::Invalid(format!(
            "{} would land outside the solution",
            target.display()
        ))
    })?;
    let mut path = root.to_path_buf();
    for component in relative.components() {
        path.push(component);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if is_link_or_junction(&metadata) => {
                return Err(NewBlockError::Invalid(format!(
                    "{} is a symlink or junction; the building block folder must stay inside the solution",
                    path.display()
                )))
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(NewBlockError::Io { path, why: error.to_string() }),
        }
    }
    Ok(())
}

fn unreadable_is_under(unreadable: &str, directory: &Path) -> bool {
    let normalize = |text: String| {
        let mut text = text.replace('\\', "/");
        while text.contains("/./") {
            text = text.replace("/./", "/");
        }
        #[cfg(windows)]
        let text = text.to_lowercase();
        text
    };
    let directory = normalize(directory.display().to_string());
    let unreadable = normalize(unreadable.to_string());
    unreadable
        .strip_prefix(&directory)
        .is_some_and(|rest| rest.starts_with(':') || rest.starts_with('/'))
}

/// The `PTC.Base` extension another project of this solution declares, from its Project XML.
fn declared_base_extension(solution: &Solution) -> Option<String> {
    for project in &solution.projects {
        let path = solution
            .project_root(project)
            .join("Projects")
            .join(format!("{}.xml", project.name));
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let Some(at) = text.find("PTC.Base:") else {
            continue;
        };
        let tail = &text[at..];
        let end = tail.find([',', '&', '"']).unwrap_or(tail.len());
        let found = &tail[..end];
        // Only an extension version (`PTC.Base:10.1.0`), not the project dependency `PTC.Base:0.0.0`.
        if found != "PTC.Base:0.0.0" {
            return Some(found.to_string());
        }
        // A later `PTC.Base:` in the same file may be the extension.
        if let Some(next) = tail[found.len()..].find("PTC.Base:") {
            let tail = &tail[found.len() + next..];
            let end = tail.find([',', '&', '"']).unwrap_or(tail.len());
            return Some(tail[..end].to_string());
        }
    }
    None
}

pub fn plan(solution: &Solution, request: &Request) -> Result<Plan, NewBlockError> {
    let invalid = |why: String| NewBlockError::Invalid(why);
    refs::validate_new_name(&request.name).map_err(invalid)?;
    if !request.name.contains('.') {
        return Err(NewBlockError::Invalid(format!(
            "{} is a single word; a building block is named with a namespace, such as Acme.Orders",
            request.name
        )));
    }
    let root = request.root.clone().unwrap_or_else(|| request.name.clone());
    if root.is_empty()
        || root.starts_with('/')
        || root.starts_with('\\')
        || root.contains("..")
        || root.contains(':')
        || root.contains(['"', '\\'])
        || root.chars().any(char::is_control)
    {
        return Err(invalid(format!("the project folder {root:?} must be a relative path inside the solution without quotes, backslashes, or control characters")));
    }
    if let Some(project) = solution
        .projects
        .iter()
        .find(|project| roots_equal(&project.root, &root))
    {
        return Err(invalid(format!(
            "the project folder {root:?} is already used by project {} in twaco.toml",
            project.name
        )));
    }
    for (what, value) in [
        ("description", request.description.as_str()),
        (
            "display name",
            request.display_name.as_deref().unwrap_or(""),
        ),
    ] {
        if value.contains(['\n', '\r']) {
            return Err(invalid(format!("the {what} must be one line")));
        }
        if value.contains("]]>") {
            return Err(invalid(format!("the {what} must not contain ]]>")));
        }
    }
    match (request.kind, request.parent.as_deref()) {
        (BlockType::Implementation, None) => {
            return Err(invalid(
                "an implementation names the abstract block it implements: give --parent"
                    .to_string(),
            ))
        }
        (BlockType::Implementation, Some(parent)) => {
            refs::validate_new_name(parent).map_err(invalid)?;
            if parent == request.name {
                return Err(invalid("a block cannot implement itself".to_string()));
            }
        }
        (_, Some(_)) => {
            return Err(invalid(
                "--parent is only for an implementation".to_string(),
            ))
        }
        _ => {}
    }
    if request.kind != BlockType::Implementation && !request.management_shape {
        return Err(invalid(
            "only an implementation may leave out the management shape".to_string(),
        ));
    }
    if solution.project(&request.name).is_some() {
        return Err(NewBlockError::Exists(vec![format!(
            "project {} in twaco.toml",
            request.name
        )]));
    }

    let mut notes = Vec::new();
    // What the project depends on.
    let depends = match request.kind {
        BlockType::Standard | BlockType::Abstract => {
            let extension = request.base_extension.clone().or_else(|| declared_base_extension(solution)).ok_or_else(|| {
                invalid("no PTC.Base version to depend on: another project of this solution declares none; pass --base-extension PTC.Base:<version> (the version installed on your server)".to_string())
            })?;
            if !extension.starts_with("PTC.Base:") {
                return Err(invalid(format!(
                    "--base-extension must look like PTC.Base:10.1.0, not {extension:?}"
                )));
            }
            format!("{{\"extensions\":\"{extension}\",\"projects\":\"PTC.Base:0.0.0\"}}")
        }
        BlockType::Implementation => {
            let parent = request
                .parent
                .as_deref()
                .expect("an implementation has a parent");
            format!("{{\"extensions\":\"\",\"projects\":\"{parent}:1.0.0\"}}")
        }
    };
    let parent_in_solution = request
        .parent
        .as_deref()
        .filter(|parent| solution.project(parent).is_some());
    if let Some(parent) = request.parent.as_deref() {
        if parent_in_solution.is_none() {
            notes.push(format!("{parent} is not a project of this solution: its EntryPoint_TT and Manager_TT must already be on the server when this block is deployed."));
        }
    }

    let (entry_base, manager_base) = match request.parent.as_deref() {
        Some(parent) => (
            format!("{parent}.EntryPoint_TT"),
            format!("{parent}.Manager_TT"),
        ),
        None => (
            "PTC.Base.ComponentEntryPoint_TT".to_string(),
            "PTC.Base.CommonManager_TT".to_string(),
        ),
    };
    let display = request
        .display_name
        .clone()
        .unwrap_or_else(|| request.name.clone());
    let mut values: BTreeMap<&str, String> = BTreeMap::new();
    values.insert("NAME", request.name.clone());
    values.insert("DESCRIPTION", attribute_escaped(&request.description));
    values.insert("DESCRIPTION_TEXT", request.description.clone());
    values.insert("DISPLAY_TEXT", display);
    values.insert("TYPE", request.kind.component_type().to_string());
    values.insert("ENTRY_BASE", entry_base);
    values.insert("MANAGER_BASE", manager_base);
    values.insert("DEPENDS_ON", attribute_escaped(&depends));

    let base = solution.root.join(&root);
    refuse_linked_path(&solution.root, &base)?;

    let manager_template = if request.management_shape {
        MANAGER_TEMPLATE.to_string()
    } else {
        let replaced = MANAGER_TEMPLATE.replace(
            MANAGER_SHAPE_BLOCK,
            "            <ImplementedShapes></ImplementedShapes>",
        );
        assert_ne!(
            replaced, MANAGER_TEMPLATE,
            "the manager template lists its shape"
        );
        replaced
    };
    let mut entities: Vec<(&str, String, &str)> = vec![
        ("Projects", request.name.clone(), PROJECT),
        (
            "Groups",
            format!("{}.Default_UG", request.name),
            DEFAULT_GROUP,
        ),
        ("Groups", format!("{}.Admin_UG", request.name), ADMIN_GROUP),
        (
            "Organizations",
            format!("{}.Default_OR", request.name),
            ORGANIZATION,
        ),
        (
            "ThingTemplates",
            format!("{}.EntryPoint_TT", request.name),
            ENTRY_POINT_TEMPLATE,
        ),
        (
            "Things",
            format!("{}.EntryPoint", request.name),
            ENTRY_POINT,
        ),
        ("ThingTemplates", format!("{}.Manager_TT", request.name), ""),
    ];
    if request.management_shape {
        entities.push((
            "ThingShapes",
            format!("{}.Management_TS", request.name),
            MANAGEMENT_SHAPE,
        ));
    }
    if request.model_logic {
        entities.push((
            "ThingShapes",
            format!("{}.ModelLogic_TS", request.name),
            MODEL_LOGIC_SHAPE,
        ));
    }
    if request.kind != BlockType::Abstract {
        entities.push(("Things", format!("{}.Manager", request.name), MANAGER));
    }
    let longest_file_name = entities
        .iter()
        .map(|(_, name, _)| format!("{name}.xml"))
        .max_by_key(|name| name.chars().count())
        .expect("a building block has entities");
    let longest_chars = longest_file_name.chars().count();
    if longest_chars > MAX_FILE_NAME_CHARS {
        return Err(invalid(format!(
            "the generated file name {longest_file_name:?} is {longest_chars} characters; file names may not exceed {MAX_FILE_NAME_CHARS} characters"
        )));
    }
    let mut files = Vec::new();
    for (collection, name, template) in entities {
        let template = if template.is_empty() {
            manager_template.as_str()
        } else {
            template
        };
        files.push(NewFile {
            path: base.join(collection).join(format!("{name}.xml")),
            text: fill(template, &values),
        });
    }

    // Nothing that exists is replaced.
    let found = workspace::discover(solution);
    let mut taken = Vec::new();
    for file in &files {
        if file.path.exists() {
            taken.push(
                file.path
                    .strip_prefix(&solution.root)
                    .unwrap_or(&file.path)
                    .display()
                    .to_string()
                    .replace('\\', "/"),
            );
        }
    }
    let wanted: Vec<String> = files
        .iter()
        .map(|file| {
            file.path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
        .collect();
    for entity in &found.entities {
        if wanted.contains(&entity.info.name) {
            taken.push(format!(
                "{}/{} (in {})",
                entity.info.collection,
                entity.info.name,
                entity
                    .path
                    .strip_prefix(&solution.root)
                    .unwrap_or(&entity.path)
                    .display()
                    .to_string()
                    .replace('\\', "/")
            ));
        }
    }
    if let Some(unreadable) = found.unreadable.iter().find(|unreadable| {
        unreadable_is_under(unreadable, &base)
            || wanted.iter().any(|name| unreadable.contains(name))
    }) {
        return Err(invalid(format!(
            "cannot check building block collisions because this file is unreadable: {unreadable}"
        )));
    }
    taken.sort();
    taken.dedup();
    if !taken.is_empty() {
        return Err(NewBlockError::Exists(taken));
    }
    for (path, _) in files.iter().map(|file| (&file.path, ())) {
        let relative = path.strip_prefix(&solution.root).unwrap_or(path);
        if relative.components().any(|part| part.as_os_str() == "..") {
            return Err(invalid(
                "a file would land outside the solution".to_string(),
            ));
        }
    }

    // The project entry in twaco.toml.
    let config_path = solution.root.join(CONFIG_FILE);
    let before = std::fs::read_to_string(&config_path).map_err(|error| NewBlockError::Io {
        path: config_path.clone(),
        why: error.to_string(),
    })?;
    let mut addition = format!(
        "\n[[project]]\nname = \"{}\"\nroot = \"{}\"\n",
        request.name,
        root.replace('\\', "/")
    );
    if let Some(parent) = parent_in_solution {
        addition.push_str(&format!("depends_on = [\"{parent}\"]\n"));
    }
    let separator = if before.ends_with('\n') { "" } else { "\n" };
    let after = format!("{before}{separator}{addition}");
    notes.push("The component permission helper is not created: the framework builds it on the server; run `GetComponentPermissionsHelper` there when the block is deployed.".to_string());
    Ok(Plan {
        request: request.clone(),
        root,
        files,
        config_before: before,
        config_after: after,
        config_addition: addition,
        notes,
    })
}

/// Create the files (never over an existing one) and append the project to `twaco.toml`; a failure
/// removes what was created and restores the configuration.
pub fn apply(solution: &Solution, plan: &Plan) -> Result<Vec<PathBuf>, NewBlockError> {
    let config_path = solution.root.join(CONFIG_FILE);
    let io = |path: &Path, error: std::io::Error| NewBlockError::Io {
        path: path.to_path_buf(),
        why: error.to_string(),
    };
    let mut created: Vec<PathBuf> = Vec::new();
    let mut created_dirs: Vec<PathBuf> = Vec::new();
    let mut config_written = false;
    let result = (|| -> Result<(), NewBlockError> {
        refuse_linked_path(&solution.root, &solution.root.join(&plan.root))?;
        for file in &plan.files {
            if let Some(parent) = file.path.parent() {
                refuse_linked_path(&solution.root, parent)?;
                let mut missing = Vec::new();
                let mut at = parent;
                while !at.exists() {
                    missing.push(at.to_path_buf());
                    match at.parent() {
                        Some(next) => at = next,
                        None => break,
                    }
                }
                std::fs::create_dir_all(parent).map_err(|error| io(parent, error))?;
                missing.reverse();
                created_dirs.extend(missing);
            }
            match std::fs::symlink_metadata(&file.path) {
                Ok(_) => {
                    return Err(io(
                        &file.path,
                        std::io::Error::new(std::io::ErrorKind::AlreadyExists, "file exists"),
                    ))
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(io(&file.path, error)),
            }
            workspace::atomic_replace(&file.path, file.text.as_bytes())
                .map_err(|error| io(&file.path, error))?;
            created.push(file.path.clone());
        }
        match std::fs::read_to_string(&config_path) {
            Ok(current) if current == plan.config_before => {}
            Ok(_) => {
                return Err(NewBlockError::Invalid(
                    "twaco.toml changed since the plan; run again".to_string(),
                ))
            }
            Err(error) => return Err(io(&config_path, error)),
        }
        workspace::atomic_replace(&config_path, plan.config_after.as_bytes())
            .map_err(|error| io(&config_path, error))?;
        config_written = true;
        Ok(())
    })();
    if let Err(error) = result {
        for path in created.iter().rev() {
            let _ = std::fs::remove_file(path);
        }
        for dir in created_dirs.iter().rev() {
            let _ = std::fs::remove_dir(dir);
        }
        if config_written
            && std::fs::read_to_string(&config_path).ok().as_deref()
                == Some(plan.config_after.as_str())
        {
            let _ = workspace::atomic_replace(&config_path, plan.config_before.as_bytes());
        }
        return Err(error);
    }
    Ok(created)
}

#[cfg(test)]
mod tests;
