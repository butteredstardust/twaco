//! A server's file repositories: listed, read, and compared with a tree kept in source control.
//!
//! A FileRepository is a Thing whose own services list and change its files.
//! Composer's Repository page lists a folder with `ListDirectories` and `GetFileListing`, and
//! downloads through the `FileRepositories` servlet; twaco does the same. Projects commit a
//! repository's content under `filerepository/<repo>/`, so the useful
//! question is how that tree and the server's differ.

use super::entity_key::ServiceTarget;
use super::server::{Client, ServerError};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(300);
/// Where a repository's tree lives in a solution.
pub const DEFAULT_ROOT: &str = "filerepository";

/// What this module asks of a server, as a trait so it is tested offline. `Sync`, because
/// status downloads in parallel.
pub trait Remote: Sync {
    fn service(
        &self,
        repository: &str,
        service: &str,
        body: &Value,
    ) -> Result<Option<Value>, ServerError>;
    fn download(&self, repository: &str, path: &str) -> Result<Vec<u8>, ServerError>;
    fn repositories(&self) -> Result<Vec<String>, ServerError>;
}

impl Remote for Client {
    fn service(
        &self,
        repository: &str,
        service: &str,
        body: &Value,
    ) -> Result<Option<Value>, ServerError> {
        let target = ServiceTarget::entity("Things", repository)?;
        self.call_service(&target, service, body, TIMEOUT)
    }

    fn download(&self, repository: &str, path: &str) -> Result<Vec<u8>, ServerError> {
        self.download_file(repository, path)
    }

    fn repositories(&self) -> Result<Vec<String>, ServerError> {
        let reply = self.call_service(
            &ServiceTarget::platform("ThingTemplates", "FileRepository"),
            "GetImplementingThings",
            &json!({}),
            TIMEOUT,
        )?;
        let mut names: Vec<String> = reply
            .as_ref()
            .and_then(|value| value.get("rows"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|row| row.get("name").and_then(Value::as_str).map(str::to_string))
            .collect();
        names.sort();
        Ok(names)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    #[error("{0}")]
    Remote(ServerError),
    #[error("unexpected repository response: {0}")]
    Shape(String),
    #[error("{0}")]
    Invalid(String),
    #[error("{}: {why}", .path.display())]
    Local { path: PathBuf, why: String },
}

/// A repository path as the server takes it: `/`-rooted, slash-separated, with no empty, `.`
/// or `..` segment. The server refuses a climbing path itself; refusing it here says so plainly.
pub fn remote_path(text: &str) -> Result<String, RepoError> {
    let segments: Vec<&str> = text
        .split(['/', '\\'])
        .filter(|segment| !segment.is_empty())
        .collect();
    if segments
        .iter()
        .any(|segment| *segment == "." || *segment == "..")
    {
        return Err(RepoError::Invalid(format!(
            "{text:?} leaves the repository; paths are inside it, such as /Thumbnails/a.png"
        )));
    }
    Ok(format!("/{}", segments.join("/")))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    /// From the repository root, `/`-rooted.
    pub path: String,
    pub size: u64,
    /// Epoch milliseconds.
    pub modified: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Listing {
    pub folders: Vec<String>,
    pub files: Vec<File>,
}

fn rows(reply: Option<Value>, what: &str) -> Result<Vec<Value>, RepoError> {
    reply
        .and_then(|value| value.get("rows").and_then(Value::as_array).cloned())
        .ok_or_else(|| RepoError::Shape(format!("{what} returned no rows")))
}

fn text(row: &Value, key: &str) -> String {
    row.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One folder, or with `recursive` everything under it, sorted by path.
pub fn list(
    remote: &dyn Remote,
    repository: &str,
    folder: &str,
    recursive: bool,
) -> Result<Listing, RepoError> {
    let folder = remote_path(folder)?;
    let mut listing = Listing::default();
    let mut visited = std::collections::BTreeSet::new();
    let mut pending = vec![folder];
    while let Some(folder) = pending.pop() {
        if !visited.insert(folder.clone()) {
            continue;
        }
        let body = json!({ "path": folder });
        // A path the server lists is where a pull writes, so it is checked like one typed in:
        // no climbing, and under the folder it was listed in.
        let inside = |path: String| -> Result<String, RepoError> {
            let checked = remote_path(&path).map_err(|_| {
                RepoError::Shape(format!(
                    "the server listed {path:?} in {folder}, which leaves the repository"
                ))
            })?;
            let prefix = if folder == "/" {
                "/".to_string()
            } else {
                format!("{folder}/")
            };
            if !checked.starts_with(&prefix) || checked == folder {
                return Err(RepoError::Shape(format!(
                    "the server listed {path:?} in {folder}, which is not inside it"
                )));
            }
            Ok(checked)
        };
        let folders = rows(
            remote
                .service(repository, "ListDirectories", &body)
                .map_err(RepoError::Remote)?,
            "ListDirectories",
        )?;
        for row in &folders {
            let path = inside(text(row, "path"))?;
            if recursive {
                pending.push(path.clone());
            }
            listing.folders.push(path);
        }
        let files = rows(
            remote
                .service(repository, "GetFileListing", &body)
                .map_err(RepoError::Remote)?,
            "GetFileListing",
        )?;
        for row in &files {
            listing.files.push(File {
                path: inside(text(row, "path"))?,
                size: row.get("size").and_then(Value::as_f64).unwrap_or(0.0) as u64,
                modified: row
                    .get("lastModifiedDate")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0) as i64,
            });
        }
    }
    listing.folders.sort();
    listing.files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(listing)
}

/// A file's bytes, exactly as stored.
pub fn get(remote: &dyn Remote, repository: &str, path: &str) -> Result<Vec<u8>, RepoError> {
    let path = remote_path(path)?;
    if path == "/" {
        return Err(RepoError::Invalid(
            "give a file's path, not the repository root".to_string(),
        ));
    }
    remote
        .download(repository, &path)
        .map_err(RepoError::Remote)
}

// ---- single changes, planned before they are made ---------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Folder,
}

/// Whether a path exists, and as what. The server answers a missing path with a 404.
pub fn kind(
    remote: &dyn Remote,
    repository: &str,
    path: &str,
) -> Result<Option<(Kind, u64)>, RepoError> {
    match remote.service(repository, "GetFileInfo", &json!({ "path": path })) {
        Ok(reply) => {
            let row = rows(reply, "GetFileInfo")?.into_iter().next();
            Ok(row.map(|row| {
                let kind = if text(&row, "fileType") == "D" {
                    Kind::Folder
                } else {
                    Kind::File
                };
                (
                    kind,
                    row.get("size").and_then(Value::as_f64).unwrap_or(0.0) as u64,
                )
            }))
        }
        Err(error) if error.is_not_found() => Ok(None),
        Err(error) => Err(RepoError::Remote(error)),
    }
}

/// One change to a repository. Every guard here exists because the server lacks it, as
/// measured on a throwaway repository: SaveBinary overwrites silently, DeleteFolder removes a
/// non-empty folder whole, CreateFolder on an existing folder is a server error, and DeleteFile
/// or DeleteFolder on the other kind is one too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Put {
        path: String,
        bytes: Vec<u8>,
        overwrite: bool,
    },
    Mkdir {
        path: String,
    },
    Remove {
        path: String,
        recursive: bool,
    },
    Move {
        from: String,
        to: String,
        overwrite: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    /// What the change does, in words.
    pub plan: String,
    /// Nothing to do: the repository already says what was asked.
    pub nothing: bool,
    pub applied: bool,
}

/// Check a change against the repository, and unless `apply`, stop at the plan. Applied, the
/// result is read back: an upload must download as the same bytes, a removed path must be
/// gone, a moved file must be at its target and not at its source.
pub fn change(
    remote: &dyn Remote,
    repository: &str,
    change: &Change,
    apply: bool,
) -> Result<Planned, RepoError> {
    let refuse = |why: String| Err(RepoError::Invalid(format!("{why}; nothing was sent")));
    let mut planned_kind = None;
    let (plan, nothing) = match change {
        Change::Put {
            path,
            bytes,
            overwrite,
        } => match kind(remote, repository, path)? {
            Some((Kind::Folder, _)) => return refuse(format!("{path} is a folder")),
            Some((Kind::File, size)) if !overwrite => {
                return refuse(format!(
                    "{path} exists ({size} bytes); pass --overwrite to replace it"
                ))
            }
            Some((Kind::File, size)) => (
                format!("replace {path} ({size} bytes) with {} bytes", bytes.len()),
                false,
            ),
            None => (format!("upload {} bytes to {path}", bytes.len()), false),
        },
        Change::Mkdir { path } => match kind(remote, repository, path)? {
            Some((Kind::Folder, _)) => (format!("{path} already exists"), true),
            Some((Kind::File, _)) => return refuse(format!("{path} is a file")),
            None => (format!("create the folder {path}"), false),
        },
        Change::Remove { path, recursive } => match kind(remote, repository, path)? {
            None => return refuse(format!("{path} does not exist")),
            Some((Kind::File, size)) => {
                planned_kind = Some(Kind::File);
                (format!("delete {path} ({size} bytes)"), false)
            }
            Some((Kind::Folder, _)) => {
                planned_kind = Some(Kind::Folder);
                let inside = list(remote, repository, path, true)?;
                let (files, folders) = (inside.files.len(), inside.folders.len());
                if files + folders > 0 && !recursive {
                    return refuse(format!(
                        "{path} holds {files} file(s) and {folders} folder(s); pass --recursive to delete them all"
                    ));
                }
                (format!("delete the folder {path} with {files} file(s) and {folders} folder(s) in it"), false)
            }
        },
        Change::Move {
            from,
            to,
            overwrite,
        } => {
            match kind(remote, repository, from)? {
                None => return refuse(format!("{from} does not exist")),
                Some((Kind::Folder, _)) => {
                    return refuse(format!("{from} is a folder; only files can be moved"))
                }
                Some((Kind::File, _)) => {}
            }
            match kind(remote, repository, to)? {
                Some((Kind::Folder, _)) => {
                    return refuse(format!(
                        "{to} is a folder; give the file's new path, such as {to}/name"
                    ))
                }
                Some((Kind::File, size)) if !overwrite => {
                    return refuse(format!(
                        "{to} exists ({size} bytes); pass --overwrite to replace it"
                    ))
                }
                Some((Kind::File, _)) => (format!("move {from} to {to}, replacing it"), false),
                None => (format!("move {from} to {to}"), false),
            }
        }
    };
    if !apply || nothing {
        return Ok(Planned {
            plan,
            nothing,
            applied: false,
        });
    }
    let sent = |service: &str, body: Value| {
        remote
            .service(repository, service, &body)
            .map(|_| ())
            .map_err(RepoError::Remote)
    };
    let failed = |why: String| Err(RepoError::Shape(format!("{plan} was sent, but {why}")));
    match change {
        Change::Put { path, bytes, .. } => {
            use base64::Engine;
            sent(
                "SaveBinary",
                json!({ "path": path, "content": base64::engine::general_purpose::STANDARD.encode(bytes) }),
            )?;
            if remote
                .download(repository, path)
                .map_err(RepoError::Remote)?
                != *bytes
            {
                return failed(format!("{path} does not download as the bytes sent"));
            }
        }
        Change::Mkdir { path } => {
            sent("CreateFolder", json!({ "path": path }))?;
            if kind(remote, repository, path)?.map(|(k, _)| k) != Some(Kind::Folder) {
                return failed(format!("{path} is not a folder afterwards"));
            }
        }
        Change::Remove { path, .. } => {
            // The kind the plan checked, not a second look: a file replaced by a full folder
            // in between must not be deleted as a folder without --recursive.
            let service = match planned_kind {
                Some(Kind::Folder) => "DeleteFolder",
                _ => "DeleteFile",
            };
            if kind(remote, repository, path)?.map(|(k, _)| k) != planned_kind {
                return Err(RepoError::Invalid(format!(
                    "{path} changed since the plan was made; nothing was sent, plan again"
                )));
            }
            sent(service, json!({ "path": path }))?;
            if kind(remote, repository, path)?.is_some() {
                return failed(format!("{path} still exists"));
            }
        }
        Change::Move {
            from,
            to,
            overwrite,
        } => {
            sent(
                "MoveFile",
                json!({ "sourcePath": from, "targetPath": to, "overwrite": overwrite }),
            )?;
            if kind(remote, repository, from)?.is_some() || kind(remote, repository, to)?.is_none()
            {
                return failed(format!("{to} is not there, or {from} still is"));
            }
        }
    }
    Ok(Planned {
        plan,
        nothing,
        applied: true,
    })
}

// ---- status: a local tree against the server -------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum State {
    Same,
    Differs,
    LocalOnly,
    RemoteOnly,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            State::Same => "same",
            State::Differs => "differs",
            State::LocalOnly => "local-only",
            State::RemoteOnly => "remote-only",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compared {
    pub path: String,
    pub state: State,
    pub local_size: Option<u64>,
    pub remote_size: Option<u64>,
}

/// Every file under a local folder, by its `/`-rooted path relative to it. Hidden files and
/// folders (a leading `.`) are not repository content.
pub fn local_files(root: &Path) -> Result<BTreeMap<String, PathBuf>, RepoError> {
    let mut found = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(folder) = pending.pop() {
        let entries = std::fs::read_dir(&folder).map_err(|e| RepoError::Local {
            path: folder.clone(),
            why: e.to_string(),
        })?;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file() {
                let relative = path.strip_prefix(root).expect("walked from root");
                let key = format!("/{}", relative.to_string_lossy().replace('\\', "/"));
                found.insert(key, path);
            }
        }
    }
    Ok(found)
}

/// The local tree against the server's, file by file. Sizes decide first; equal sizes are
/// settled by downloading and hashing, so `same` means the same bytes.
pub fn status(
    remote: &dyn Remote,
    repository: &str,
    local_root: &Path,
) -> Result<Vec<Compared>, RepoError> {
    let local = if local_root.is_dir() {
        local_files(local_root)?
    } else {
        BTreeMap::new()
    };
    let remote_files: BTreeMap<String, u64> = list(remote, repository, "/", true)?
        .files
        .into_iter()
        .map(|file| (file.path, file.size))
        .collect();
    let mut paths: Vec<&String> = local.keys().chain(remote_files.keys()).collect();
    paths.sort();
    paths.dedup();
    let mut out = Vec::new();
    // Equal sizes are settled by content, downloaded in parallel: a tree of thumbnails is
    // mostly equal sizes, and one at a time took 8.5 s for 28 files.
    let mut to_hash: Vec<usize> = Vec::new();
    for path in paths {
        let local_size = match local.get(path) {
            Some(file) => Some(
                std::fs::metadata(file)
                    .map_err(|e| RepoError::Local {
                        path: file.clone(),
                        why: e.to_string(),
                    })?
                    .len(),
            ),
            None => None,
        };
        let remote_size = remote_files.get(path).copied();
        let state = match (local_size, remote_size) {
            (Some(_), None) => State::LocalOnly,
            (None, Some(_)) => State::RemoteOnly,
            (Some(l), Some(r)) if l != r => State::Differs,
            (Some(_), Some(_)) => {
                to_hash.push(out.len());
                State::Same
            }
            (None, None) => continue,
        };
        out.push(Compared {
            path: path.clone(),
            state,
            local_size,
            remote_size,
        });
    }
    let verdicts = super::parallel::map(&to_hash, |&at| -> Result<bool, RepoError> {
        let path = &out[at].path;
        let file = &local[path];
        let mine = std::fs::read(file).map_err(|e| RepoError::Local {
            path: file.clone(),
            why: e.to_string(),
        })?;
        let theirs = remote
            .download(repository, path)
            .map_err(RepoError::Remote)?;
        Ok(Sha256::digest(&mine) == Sha256::digest(&theirs))
    });
    for (at, verdict) in to_hash.into_iter().zip(verdicts) {
        if !verdict? {
            out[at].state = State::Differs;
        }
    }
    Ok(out)
}

// ---- tree sync -------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// The local tree to the server.
    Push,
    /// The server to the local tree.
    Pull,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Synced {
    /// Paths copied, or to be copied by a plan.
    pub copied: Vec<String>,
    /// Paths on the far side only, which a sync never deletes.
    pub left: Vec<String>,
    pub same: usize,
    pub applied: bool,
}

/// Copy what one side has that the other lacks, or holds differently. Never deletes, on
/// either side: a file present only where the copy goes is reported in `left`. A file that
/// differs on both sides is a conflict, and any conflict refuses the whole sync unless
/// `overwrite`, so a sync never half-happens over a decision nobody made.
pub fn sync(
    remote: &dyn Remote,
    repository: &str,
    local_root: &Path,
    direction: Direction,
    overwrite: bool,
    apply: bool,
) -> Result<Synced, RepoError> {
    let compared = status(remote, repository, local_root)?;
    let (source_only, target_only) = match direction {
        Direction::Push => (State::LocalOnly, State::RemoteOnly),
        Direction::Pull => (State::RemoteOnly, State::LocalOnly),
    };
    // Two server paths that differ only in case are one file on Windows and macOS: a pull would
    // write both there and keep whichever came last, and a checkout of the solution would fail
    // the same way on such a machine. Refuse rather than lose one.
    if direction == Direction::Pull {
        let clashes = case_clashes(&compared);
        if !clashes.is_empty() {
            return Err(RepoError::Invalid(format!(
                "paths that differ only in case ({}) are one file or folder on Windows and macOS; \
                 rename one on the server or locally; nothing was copied",
                clashes.join("; ")
            )));
        }
    }
    let conflicts: Vec<&str> = compared
        .iter()
        .filter(|c| c.state == State::Differs)
        .map(|c| c.path.as_str())
        .collect();
    if !conflicts.is_empty() && !overwrite {
        return Err(RepoError::Invalid(format!(
            "{} file(s) differ on both sides ({}); pass --overwrite to replace the {} copies; nothing was copied",
            conflicts.len(),
            conflicts.iter().take(5).copied().collect::<Vec<_>>().join(", "),
            if direction == Direction::Push { "server's" } else { "local" }
        )));
    }
    let mut synced = Synced {
        same: compared.iter().filter(|c| c.state == State::Same).count(),
        ..Synced::default()
    };
    let mut differs = std::collections::BTreeSet::new();
    for item in &compared {
        if item.state == source_only || item.state == State::Differs {
            synced.copied.push(item.path.clone());
            if item.state == State::Differs {
                differs.insert(item.path.clone());
            }
        } else if item.state == target_only {
            synced.left.push(item.path.clone());
        }
    }
    if !apply {
        return Ok(synced);
    }
    let mut done: Vec<String> = Vec::new();
    for path in &synced.copied {
        let result = copy_one(
            remote,
            repository,
            local_root,
            direction,
            path,
            differs.contains(path),
        );
        if let Err(error) = result {
            return Err(RepoError::Invalid(format!(
                "{} of {} file(s) were copied before {path} failed ({error}); copied: {}",
                done.len(),
                synced.copied.len(),
                if done.is_empty() {
                    "none".to_string()
                } else {
                    done.join(", ")
                }
            )));
        }
        done.push(path.clone());
    }
    synced.applied = true;
    Ok(synced)
}

/// Pairs a pull would write to one place on a case-insensitive filesystem: a server path and any
/// other path, the server's or a local one, equal apart from case, or one naming a folder of the
/// other apart from case (`/A` and `/a/x`). Unicode normalisation (a macOS volume storing `é`
/// decomposed) is not compared.
fn case_clashes(compared: &[Compared]) -> Vec<String> {
    let lowered: Vec<(String, &Compared)> = compared
        .iter()
        .map(|item| (item.path.to_lowercase(), item))
        .collect();
    let mut clashes = Vec::new();
    for (index, (lower, item)) in lowered.iter().enumerate() {
        for (other_lower, other) in &lowered[index + 1..] {
            // Only what the pull writes can clash; two local files are the local disk's business.
            if item.state == State::LocalOnly && other.state == State::LocalOnly {
                continue;
            }
            let same = lower == other_lower;
            let nested = other_lower.starts_with(&format!("{lower}/"))
                && !other.path.starts_with(&format!("{}/", item.path))
                || lower.starts_with(&format!("{other_lower}/"))
                    && !item.path.starts_with(&format!("{}/", other.path));
            if same || nested {
                clashes.push(format!("{} and {}", item.path, other.path));
            }
        }
    }
    clashes
}

/// One file of a sync. A push replaces a server file only when the plan found it differing:
/// one that appeared since is not overwritten. A pull writes only inside the local tree, and
/// never through a link.
fn copy_one(
    remote: &dyn Remote,
    repository: &str,
    local_root: &Path,
    direction: Direction,
    path: &str,
    planned_differing: bool,
) -> Result<(), RepoError> {
    let path = remote_path(path)?;
    let local = local_root.join(
        path.trim_start_matches('/')
            .replace('/', std::path::MAIN_SEPARATOR_STR),
    );
    match direction {
        Direction::Push => {
            let bytes = std::fs::read(&local).map_err(|e| RepoError::Local {
                path: local.clone(),
                why: e.to_string(),
            })?;
            change(
                remote,
                repository,
                &Change::Put {
                    path,
                    bytes,
                    overwrite: planned_differing,
                },
                true,
            )?;
        }
        Direction::Pull => {
            let bytes = remote
                .download(repository, &path)
                .map_err(RepoError::Remote)?;
            let folder = local.parent().expect("a file has a folder");
            std::fs::create_dir_all(folder).map_err(|e| RepoError::Local {
                path: folder.to_path_buf(),
                why: e.to_string(),
            })?;
            let root = std::fs::canonicalize(local_root).map_err(|e| RepoError::Local {
                path: local_root.to_path_buf(),
                why: e.to_string(),
            })?;
            let real = std::fs::canonicalize(folder).map_err(|e| RepoError::Local {
                path: folder.to_path_buf(),
                why: e.to_string(),
            })?;
            if !real.starts_with(&root) {
                return Err(RepoError::Local {
                    path: folder.to_path_buf(),
                    why: "leads outside the repository's tree (a link?)".to_string(),
                });
            }
            if std::fs::symlink_metadata(&local).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err(RepoError::Local {
                    path: local.clone(),
                    why: "is a link; twaco does not write through one".to_string(),
                });
            }
            super::workspace::write_entity(&local, &bytes).map_err(|e| RepoError::Local {
                path: local.clone(),
                why: e.to_string(),
            })?;
        }
    }
    Ok(())
}

/// A file's SHA-256, as lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    super::normalise::hex(&Sha256::digest(bytes))
}

/// Where a repository's tree is kept in a solution: `<root>/<repository>/`.
pub fn local_root(solution_root: &Path, configured: Option<&str>, repository: &str) -> PathBuf {
    solution_root
        .join(configured.unwrap_or(DEFAULT_ROOT))
        .join(repository)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A repository held in memory, answering as the platform was observed to.
    struct Fake {
        files: BTreeMap<String, Vec<u8>>,
        downloads: AtomicUsize,
    }

    impl Fake {
        fn with(files: &[(&str, &[u8])]) -> Self {
            Fake {
                files: files
                    .iter()
                    .map(|(p, b)| (p.to_string(), b.to_vec()))
                    .collect(),
                downloads: AtomicUsize::new(0),
            }
        }

        fn folders(&self) -> Vec<String> {
            let mut out: Vec<String> = Vec::new();
            for path in self.files.keys() {
                let mut parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
                parts.pop();
                for depth in 1..=parts.len() {
                    let folder = format!("/{}", parts[..depth].join("/"));
                    if !out.contains(&folder) {
                        out.push(folder);
                    }
                }
            }
            out
        }
    }

    fn parent(path: &str) -> String {
        match path.rfind('/') {
            Some(0) | None => "/".to_string(),
            Some(at) => path[..at].to_string(),
        }
    }

    impl Remote for Fake {
        fn service(
            &self,
            _: &str,
            service: &str,
            body: &Value,
        ) -> Result<Option<Value>, ServerError> {
            let folder = body["path"].as_str().unwrap().to_string();
            let rows: Vec<Value> = match service {
                "ListDirectories" => self
                    .folders()
                    .into_iter()
                    .filter(|f| parent(f) == folder)
                    .map(|f| json!({ "path": f, "name": f.rsplit('/').next() }))
                    .collect(),
                "GetFileListing" => self
                    .files
                    .iter()
                    .filter(|(p, _)| parent(p) == folder)
                    .map(|(p, b)| json!({ "path": p, "size": b.len() as f64, "lastModifiedDate": 1.0, "fileType": "F" }))
                    .collect(),
                other => panic!("unexpected {other}"),
            };
            Ok(Some(json!({ "rows": rows })))
        }

        fn download(&self, _: &str, path: &str) -> Result<Vec<u8>, ServerError> {
            self.downloads.fetch_add(1, Ordering::SeqCst);
            Ok(self.files[path].clone())
        }

        fn repositories(&self) -> Result<Vec<String>, ServerError> {
            Ok(vec!["R".into()])
        }
    }

    /// A repository that changes, behaving as the platform was measured to: SaveBinary
    /// overwrites, DeleteFolder removes everything under it, the wrong delete for the kind is a
    /// server error, and a missing path is a 404.
    struct Live {
        files: std::sync::Mutex<BTreeMap<String, Vec<u8>>>,
        folders: std::sync::Mutex<Vec<String>>,
        writes: std::sync::Mutex<Vec<String>>,
    }

    impl Live {
        fn with(files: &[(&str, &[u8])], folders: &[&str]) -> Self {
            Live {
                files: std::sync::Mutex::new(
                    files
                        .iter()
                        .map(|(p, b)| (p.to_string(), b.to_vec()))
                        .collect(),
                ),
                folders: std::sync::Mutex::new(folders.iter().map(|f| f.to_string()).collect()),
                writes: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    fn http(status: u16) -> ServerError {
        ServerError::Http {
            method: crate::core::server::Method::Post,
            status,
            url: "u".into(),
            body: String::new(),
        }
    }

    impl Remote for Live {
        fn service(
            &self,
            _: &str,
            service: &str,
            body: &Value,
        ) -> Result<Option<Value>, ServerError> {
            let mut files = self.files.lock().unwrap();
            let mut folders = self.folders.lock().unwrap();
            let path = |key: &str| body[key].as_str().unwrap_or_default().to_string();
            if !matches!(
                service,
                "GetFileInfo" | "ListDirectories" | "GetFileListing"
            ) {
                self.writes.lock().unwrap().push(service.to_string());
            }
            match service {
                "GetFileInfo" => {
                    let p = path("path");
                    if let Some(bytes) = files.get(&p) {
                        Ok(Some(
                            json!({ "rows": [{ "path": p, "fileType": "F", "size": bytes.len() as f64 }] }),
                        ))
                    } else if folders.contains(&p) {
                        Ok(Some(
                            json!({ "rows": [{ "path": p, "fileType": "D", "size": 0.0 }] }),
                        ))
                    } else {
                        Err(http(404))
                    }
                }
                "ListDirectories" => {
                    let p = path("path");
                    let rows: Vec<Value> = folders
                        .iter()
                        .filter(|f| parent(f) == p)
                        .map(|f| json!({ "path": f }))
                        .collect();
                    Ok(Some(json!({ "rows": rows })))
                }
                "GetFileListing" => {
                    let p = path("path");
                    let rows: Vec<Value> = files
                        .iter()
                        .filter(|(f, _)| parent(f) == p)
                        .map(|(f, b)| json!({ "path": f, "size": b.len() as f64 }))
                        .collect();
                    Ok(Some(json!({ "rows": rows })))
                }
                "SaveBinary" => {
                    use base64::Engine;
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(path("content"))
                        .unwrap();
                    files.insert(path("path"), bytes);
                    Ok(None)
                }
                "CreateFolder" if folders.contains(&path("path")) => Err(http(500)),
                "CreateFolder" => {
                    folders.push(path("path"));
                    Ok(None)
                }
                "DeleteFile" if files.remove(&path("path")).is_some() => Ok(None),
                "DeleteFile" => Err(http(500)),
                "DeleteFolder" if folders.contains(&path("path")) => {
                    let p = path("path");
                    files.retain(|f, _| !f.starts_with(&format!("{p}/")));
                    folders.retain(|f| f != &p && !f.starts_with(&format!("{p}/")));
                    Ok(None)
                }
                "DeleteFolder" => Err(http(500)),
                "MoveFile" => {
                    let (from, to) = (path("sourcePath"), path("targetPath"));
                    if files.contains_key(&to) && !body["overwrite"].as_bool().unwrap_or(false) {
                        return Err(http(406));
                    }
                    let bytes = files.remove(&from).ok_or_else(|| http(404))?;
                    files.insert(to, bytes);
                    Ok(None)
                }
                other => panic!("unexpected {other}"),
            }
        }

        fn download(&self, _: &str, path: &str) -> Result<Vec<u8>, ServerError> {
            self.files
                .lock()
                .unwrap()
                .get(path)
                .cloned()
                .ok_or_else(|| http(404))
        }

        fn repositories(&self) -> Result<Vec<String>, ServerError> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn a_change_is_a_plan_unless_applied_and_is_read_back_when_it_is() {
        let live = Live::with(&[], &[]);
        let put = Change::Put {
            path: "/A/new.bin".into(),
            bytes: b"bytes".to_vec(),
            overwrite: false,
        };
        let planned = change(&live, "R", &put, false).unwrap();
        assert_eq!(planned.plan, "upload 5 bytes to /A/new.bin");
        assert!(
            live.writes.lock().unwrap().is_empty(),
            "a plan sends nothing"
        );
        assert!(change(&live, "R", &put, true).unwrap().applied);
        assert_eq!(live.files.lock().unwrap()["/A/new.bin"], b"bytes");
    }

    #[test]
    fn nothing_is_overwritten_without_being_asked() {
        let live = Live::with(&[("/a.bin", b"old"), ("/b.bin", b"b")], &[]);
        let put = Change::Put {
            path: "/a.bin".into(),
            bytes: b"new".to_vec(),
            overwrite: false,
        };
        let error = change(&live, "R", &put, true).unwrap_err();
        assert!(error.to_string().contains("pass --overwrite"), "{error}");
        let mv = Change::Move {
            from: "/b.bin".into(),
            to: "/a.bin".into(),
            overwrite: false,
        };
        assert!(change(&live, "R", &mv, true).is_err());
        assert!(live.writes.lock().unwrap().is_empty());
        assert_eq!(live.files.lock().unwrap()["/a.bin"], b"old");

        let put = Change::Put {
            path: "/a.bin".into(),
            bytes: b"new".to_vec(),
            overwrite: true,
        };
        assert_eq!(
            change(&live, "R", &put, true).unwrap().plan,
            "replace /a.bin (3 bytes) with 3 bytes"
        );
        let mv = Change::Move {
            from: "/b.bin".into(),
            to: "/c.bin".into(),
            overwrite: false,
        };
        change(&live, "R", &mv, true).unwrap();
        assert!(live.files.lock().unwrap().contains_key("/c.bin"));
    }

    #[test]
    fn a_folder_with_content_is_deleted_only_when_that_is_asked_for() {
        let live = Live::with(&[("/F/a.bin", b"a"), ("/F/G/b.bin", b"b")], &["/F", "/F/G"]);
        let error = change(
            &live,
            "R",
            &Change::Remove {
                path: "/F".into(),
                recursive: false,
            },
            true,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("2 file(s) and 1 folder(s)"),
            "{error}"
        );
        assert!(live.writes.lock().unwrap().is_empty());
        change(
            &live,
            "R",
            &Change::Remove {
                path: "/F".into(),
                recursive: true,
            },
            true,
        )
        .unwrap();
        assert!(live.files.lock().unwrap().is_empty());
        // A file is deleted as a file, which the server requires.
        let live = Live::with(&[("/x.bin", b"x")], &[]);
        change(
            &live,
            "R",
            &Change::Remove {
                path: "/x.bin".into(),
                recursive: false,
            },
            true,
        )
        .unwrap();
        assert_eq!(*live.writes.lock().unwrap(), ["DeleteFile"]);
    }

    fn tree(files: &[(&str, &[u8])]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "twaco-sync-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        for (path, bytes) in files {
            let file = dir.join(path.trim_start_matches('/'));
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, bytes).unwrap();
        }
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn push_copies_what_is_new_and_never_deletes() {
        let dir = tree(&[("/same.bin", b"s"), ("/A/new.bin", b"n")]);
        let live = Live::with(&[("/same.bin", b"s"), ("/server-only.bin", b"r")], &[]);
        let plan = sync(&live, "R", &dir, Direction::Push, false, false).unwrap();
        assert_eq!(
            (plan.copied.clone(), plan.left.clone(), plan.same),
            (
                vec!["/A/new.bin".to_string()],
                vec!["/server-only.bin".to_string()],
                1
            )
        );
        assert!(
            live.writes.lock().unwrap().is_empty(),
            "a plan sends nothing"
        );
        sync(&live, "R", &dir, Direction::Push, false, true).unwrap();
        let files = live.files.lock().unwrap();
        assert_eq!(files["/A/new.bin"], b"n");
        assert!(files.contains_key("/server-only.bin"), "never deleted");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn pull_writes_what_is_new_locally_and_never_deletes() {
        let dir = tree(&[("/local-only.bin", b"l")]);
        let live = Live::with(&[("/B/deep/r.bin", b"remote")], &["/B", "/B/deep"]);
        sync(&live, "R", &dir, Direction::Pull, false, true).unwrap();
        assert_eq!(std::fs::read(dir.join("B/deep/r.bin")).unwrap(), b"remote");
        assert!(dir.join("local-only.bin").exists(), "never deleted");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_pull_of_paths_that_differ_only_in_case_is_refused_before_anything_is_written() {
        let dir = tree(&[]);
        let live = Live::with(&[("/A.txt", b"upper"), ("/a.txt", b"lower")], &[]);
        for apply in [false, true] {
            let error = sync(&live, "R", &dir, Direction::Pull, false, apply).unwrap_err();
            assert!(error.to_string().contains("differ only in case"), "{error}");
        }
        assert!(!dir.join("A.txt").exists() && !dir.join("a.txt").exists());
        let _ = std::fs::remove_dir_all(dir);

        // A server file and a local-only one, or a file and a folder, equal apart from case.
        let dir = tree(&[("/notes.txt", b"mine")]);
        let live = Live::with(&[("/Notes.txt", b"theirs")], &[]);
        let error = sync(&live, "R", &dir, Direction::Pull, false, false).unwrap_err();
        assert!(
            error.to_string().contains("/Notes.txt and /notes.txt"),
            "{error}"
        );
        let _ = std::fs::remove_dir_all(dir);
        let dir = tree(&[]);
        let live = Live::with(&[("/A", b"file"), ("/a/x.txt", b"in a folder")], &["/a"]);
        let error = sync(&live, "R", &dir, Direction::Pull, false, false).unwrap_err();
        assert!(error.to_string().contains("differ only in case"), "{error}");
        let _ = std::fs::remove_dir_all(dir);

        // A folder and the files in it are no clash.
        let dir = tree(&[]);
        let live = Live::with(&[("/a/x.txt", b"x"), ("/a/y.txt", b"y")], &["/a"]);
        assert!(sync(&live, "R", &dir, Direction::Pull, false, false).is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_file_that_differs_refuses_the_whole_sync_unless_overwriting() {
        let dir = tree(&[("/both.bin", b"mine"), ("/new.bin", b"n")]);
        let live = Live::with(&[("/both.bin", b"them")], &[]);
        let error = sync(&live, "R", &dir, Direction::Push, false, true).unwrap_err();
        assert!(error.to_string().contains("/both.bin"), "{error}");
        assert!(
            live.writes.lock().unwrap().is_empty(),
            "nothing copied, not even the new file"
        );
        sync(&live, "R", &dir, Direction::Pull, true, true).unwrap();
        assert_eq!(std::fs::read(dir.join("both.bin")).unwrap(), b"them");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A server whose listing names a path outside the folder it lists.
    struct Escaping;

    impl Remote for Escaping {
        fn service(&self, _: &str, service: &str, _: &Value) -> Result<Option<Value>, ServerError> {
            Ok(Some(match service {
                "ListDirectories" => json!({ "rows": [] }),
                _ => json!({ "rows": [{ "path": "/../outside.bin", "size": 1.0 }] }),
            }))
        }
        fn download(&self, _: &str, _: &str) -> Result<Vec<u8>, ServerError> {
            Ok(b"x".to_vec())
        }
        fn repositories(&self) -> Result<Vec<String>, ServerError> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn a_listed_path_that_leaves_the_repository_is_refused_before_anything_is_written() {
        let dir = tree(&[]);
        let error = sync(
            &Escaping,
            "R",
            &dir.join("repo"),
            Direction::Pull,
            false,
            true,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("leaves the repository"),
            "{error}"
        );
        assert!(!dir.join("outside.bin").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_push_does_not_replace_a_file_that_appeared_after_the_plan() {
        // Planned as local-only; by the time it is sent, the server has one.
        struct Appearing(Live);
        impl Remote for Appearing {
            fn service(
                &self,
                repository: &str,
                service: &str,
                body: &Value,
            ) -> Result<Option<Value>, ServerError> {
                if service == "GetFileInfo"
                    && body["path"] == "/new.bin"
                    && !self.0.files.lock().unwrap().contains_key("/new.bin")
                {
                    // Listing done: someone else uploads now.
                    self.0
                        .files
                        .lock()
                        .unwrap()
                        .insert("/new.bin".into(), b"theirs".to_vec());
                }
                self.0.service(repository, service, body)
            }
            fn download(&self, repository: &str, path: &str) -> Result<Vec<u8>, ServerError> {
                self.0.download(repository, path)
            }
            fn repositories(&self) -> Result<Vec<String>, ServerError> {
                Ok(Vec::new())
            }
        }
        let dir = tree(&[("/new.bin", b"mine")]);
        let remote = Appearing(Live::with(&[], &[]));
        let error = sync(&remote, "R", &dir, Direction::Push, false, true).unwrap_err();
        assert!(
            error.to_string().contains("0 of 1 file(s) were copied"),
            "{error}"
        );
        assert_eq!(
            remote.0.files.lock().unwrap()["/new.bin"],
            b"theirs",
            "not overwritten"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_existing_folder_is_not_created_again() {
        let live = Live::with(&[], &["/F"]);
        let planned = change(&live, "R", &Change::Mkdir { path: "/F".into() }, true).unwrap();
        assert!(planned.nothing && !planned.applied);
        assert!(
            live.writes.lock().unwrap().is_empty(),
            "CreateFolder on an existing folder is a server error"
        );
    }

    #[test]
    fn a_path_stays_inside_the_repository() {
        assert_eq!(
            remote_path("Thumbnails/a.png").unwrap(),
            "/Thumbnails/a.png"
        );
        assert_eq!(
            remote_path("/Thumbnails//a.png").unwrap(),
            "/Thumbnails/a.png"
        );
        assert_eq!(remote_path("\\A\\b").unwrap(), "/A/b");
        assert_eq!(remote_path("").unwrap(), "/");
        assert!(remote_path("/A/../../etc").is_err());
        assert!(remote_path("./x").is_err());
    }

    #[test]
    fn a_listing_is_one_folder_or_everything_under_it() {
        let fake = Fake::with(&[
            ("/top.txt", b"t"),
            ("/A/a.bin", b"aa"),
            ("/A/B/b.bin", b"bbb"),
        ]);
        let one = list(&fake, "R", "/", false).unwrap();
        assert_eq!(one.folders, ["/A"]);
        assert_eq!(
            one.files
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>(),
            ["/top.txt"]
        );
        let all = list(&fake, "R", "/", true).unwrap();
        assert_eq!(all.folders, ["/A", "/A/B"]);
        assert_eq!(
            all.files
                .iter()
                .map(|f| (f.path.as_str(), f.size))
                .collect::<Vec<_>>(),
            [("/A/B/b.bin", 3), ("/A/a.bin", 2), ("/top.txt", 1)]
        );
    }

    #[test]
    fn status_compares_by_size_then_by_hash_and_names_one_sided_files() {
        let dir = std::env::temp_dir().join(format!(
            "twaco-repo-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("A")).unwrap();
        std::fs::write(dir.join("A/same.bin"), b"same").unwrap();
        std::fs::write(dir.join("A/edit.bin"), b"mine").unwrap(); // same size as the server's
        std::fs::write(dir.join("A/grown.bin"), b"longer").unwrap();
        std::fs::write(dir.join("local.bin"), b"l").unwrap();
        std::fs::write(dir.join(".hidden"), b"h").unwrap();
        let fake = Fake::with(&[
            ("/A/same.bin", b"same"),
            ("/A/edit.bin", b"them"),
            ("/A/grown.bin", b"short"),
            ("/remote.bin", b"r"),
        ]);
        let states: Vec<(String, State)> = status(&fake, "R", &dir)
            .unwrap()
            .into_iter()
            .map(|c| (c.path, c.state))
            .collect();
        assert_eq!(
            states,
            [
                ("/A/edit.bin".to_string(), State::Differs),
                ("/A/grown.bin".to_string(), State::Differs),
                ("/A/same.bin".to_string(), State::Same),
                ("/local.bin".to_string(), State::LocalOnly),
                ("/remote.bin".to_string(), State::RemoteOnly),
            ]
        );
        assert_eq!(
            fake.downloads.load(Ordering::SeqCst),
            2,
            "only equal sizes are downloaded to compare"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_local_tree_is_a_listing_of_the_server() {
        let fake = Fake::with(&[("/r.bin", b"r")]);
        let states = status(&fake, "R", Path::new("Z:/no/such/tree")).unwrap();
        assert_eq!(states[0].state, State::RemoteOnly);
    }
}
