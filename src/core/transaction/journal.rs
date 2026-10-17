//! What a transaction writes down before it changes anything, and how it is read back.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Where the journals live, relative to the workspace root.
pub(crate) const DIRECTORY: &str = ".twaco/transactions";
pub(crate) const MAJOR: u32 = 1;
pub(crate) const MINOR: u32 = 0;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Format {
    pub(crate) major: u32,
    pub(crate) minor: u32,
}

/// How far an operation got. Nothing is visible until `Applying`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum State {
    /// Stage and backup files are being written; no destination has been touched.
    Staging,
    /// Every stage and backup is durable; destinations are being changed one at a time.
    Applying,
    /// Every step is in place; only the artifacts are left to remove.
    Committed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Kind {
    /// The destination holds the `before` bytes and will hold the `after` bytes.
    Replace,
    /// The destination is absent and will hold the `after` bytes.
    Create,
    /// The destination holds the `before` bytes and will be absent.
    Delete,
    /// A file or a whole folder is renamed from `path` to `to`.
    Move,
}

impl Kind {}

/// One file change. Paths are relative to the workspace root and written with `/`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Step {
    pub(crate) id: usize,
    pub(crate) kind: Kind,
    pub(crate) path: String,
    /// Digest of the destination before the step; none when it is absent.
    pub(crate) before: Option<String>,
    /// Digest of the destination after the step; none when it is absent.
    pub(crate) after: Option<String>,
    /// The bytes that will be installed, staged beside the destination.
    pub(crate) stage: Option<String>,
    /// A copy of the original bytes, kept so the step can be undone.
    pub(crate) backup: Option<String>,
    /// Where a `move` puts what is at `path`.
    #[serde(default)]
    pub(crate) to: Option<String>,
    /// For a file inside something a later `move` renames: where it is once that has happened.
    #[serde(default)]
    pub(crate) then_at: Option<String>,
    /// Folders this step created, shallowest first; removed again if the step is undone.
    #[serde(default)]
    pub(crate) new_dirs: Vec<String>,
    pub(crate) completed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Journal {
    pub(crate) format: Format,
    pub(crate) operation_id: String,
    pub(crate) command: String,
    pub(crate) state: State,
    pub(crate) steps: Vec<Step>,
}

/// `sha256:` and the lower-case hex of the digest.
pub(crate) fn digest(bytes: &[u8]) -> String {
    let mut text = String::from("sha256:");
    for byte in Sha256::digest(bytes) {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

/// The digest of a file, or of a whole folder: every regular file under it by relative path.
/// `overrides` stands for files the operation will have rewritten before the digest is taken
/// again (`None`: removed). A link anywhere inside is an error.
pub(crate) fn tree_digest(
    root: &Path,
    relative: &str,
    overrides: &std::collections::BTreeMap<String, Option<Vec<u8>>>,
) -> std::io::Result<String> {
    use std::io::{Error, ErrorKind};
    fn walk(
        root: &Path,
        relative: &str,
        overrides: &std::collections::BTreeMap<String, Option<Vec<u8>>>,
        entries: &mut std::collections::BTreeMap<String, String>,
    ) -> std::io::Result<()> {
        let path = super::paths::absolute(root, relative);
        if super::paths::is_link(&path) {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!("{relative} is or holds a link"),
            ));
        }
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            // The operation's own stages and backups sit beside the files they belong to; they
            // are not part of what is being moved.
            let mut names: Vec<String> = std::fs::read_dir(&path)?
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| {
                    !(name.starts_with('.')
                        && (name.ends_with(".twaco-stage") || name.ends_with(".twaco-backup")))
                })
                .collect();
            names.sort();
            for name in names {
                walk(root, &format!("{relative}/{name}"), overrides, entries)?;
            }
        } else if metadata.is_file() {
            let digest = match overrides.get(relative) {
                Some(Some(bytes)) => digest(bytes),
                Some(None) => return Ok(()),
                None => digest(&std::fs::read(&path)?),
            };
            entries.insert(relative.to_string(), digest);
        } else {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!("{relative} is not a file or a folder"),
            ));
        }
        Ok(())
    }
    let mut entries = std::collections::BTreeMap::new();
    walk(root, relative, overrides, &mut entries)?;
    let mut text = String::new();
    for (path, found) in entries {
        text.push_str(&format!("{path}\0{found}\n"));
    }
    Ok(format!("sha256-tree:{}", &digest(text.as_bytes())[7..]))
}

pub(crate) fn path_of(root: &Path, operation_id: &str) -> PathBuf {
    root.join(DIRECTORY).join(format!("{operation_id}.json"))
}

/// Why a journal could not be used.
#[derive(Debug)]
pub(crate) enum ReadError {
    Io(std::io::Error),
    Malformed(String),
    /// Written by a version of twaco this one does not understand.
    Unsupported {
        major: u32,
        minor: u32,
    },
}

pub(crate) fn read(path: &Path) -> Result<Journal, ReadError> {
    let bytes = std::fs::read(path).map_err(ReadError::Io)?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|why| ReadError::Malformed(why.to_string()))?;
    let major = value["format"]["major"].as_u64();
    let minor = value["format"]["minor"].as_u64().unwrap_or(0);
    match major {
        Some(found) if found == u64::from(MAJOR) => {}
        Some(found) => {
            return Err(ReadError::Unsupported {
                major: u32::try_from(found).unwrap_or(u32::MAX),
                minor: u32::try_from(minor).unwrap_or(u32::MAX),
            })
        }
        None => return Err(ReadError::Malformed("it has no format version".to_string())),
    }
    let journal: Journal =
        serde_json::from_value(value).map_err(|why| ReadError::Malformed(why.to_string()))?;
    // The operation id names files and is the journal's own file name: it may not be a path.
    let id = &journal.operation_id;
    let plain = !id.is_empty()
        && !id.starts_with('.')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    let named = path
        .file_stem()
        .is_some_and(|stem| stem.to_string_lossy() == id.as_str());
    if !plain || !named {
        return Err(ReadError::Malformed(
            "its operation id is not the name of the file that holds it".to_string(),
        ));
    }
    Ok(journal)
}

/// Replace the journal on disk in one step and make the replacement durable.
pub(crate) fn write(root: &Path, journal: &Journal) -> std::io::Result<()> {
    let folder = root.join(DIRECTORY);
    std::fs::create_dir_all(&folder)?;
    let bytes = serde_json::to_vec_pretty(journal).expect("a journal serialises");
    crate::core::workspace::atomic_replace(&path_of(root, &journal.operation_id), &bytes)?;
    sync_directory(&folder);
    Ok(())
}

/// Make a rename in `folder` durable where the platform lets a directory be synced.
pub(crate) fn sync_directory(folder: &Path) {
    #[cfg(unix)]
    if let Ok(directory) = std::fs::File::open(folder) {
        let _ = directory.sync_all();
    }
    #[cfg(not(unix))]
    let _ = folder;
}
