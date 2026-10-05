//! What a transaction writes down before it changes anything, and how it is read back.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Where the journals live, relative to the workspace root.
pub const DIRECTORY: &str = ".twaco/transactions";
pub const MAJOR: u32 = 1;
pub const MINOR: u32 = 0;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Format {
    pub major: u32,
    pub minor: u32,
}

/// How far an operation got. Nothing is visible until `Applying`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Stage and backup files are being written; no destination has been touched.
    Staging,
    /// Every stage and backup is durable; destinations are being changed one at a time.
    Applying,
    /// Every step is in place; only the artifacts are left to remove.
    Committed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// The destination holds the `before` bytes and will hold the `after` bytes.
    Replace,
    /// The destination is absent and will hold the `after` bytes.
    Create,
    /// The destination holds the `before` bytes and will be absent.
    Delete,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Replace => "replace",
            Kind::Create => "create",
            Kind::Delete => "delete",
        }
    }
}

/// One file change. Paths are relative to the workspace root and written with `/`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub id: usize,
    pub kind: Kind,
    pub path: String,
    /// Digest of the destination before the step; none when it is absent.
    pub before: Option<String>,
    /// Digest of the destination after the step; none when it is absent.
    pub after: Option<String>,
    /// The bytes that will be installed, staged beside the destination.
    pub stage: Option<String>,
    /// A copy of the original bytes, kept so the step can be undone.
    pub backup: Option<String>,
    /// Folders this step created, shallowest first; removed again if the step is undone.
    #[serde(default)]
    pub new_dirs: Vec<String>,
    pub completed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Journal {
    pub format: Format,
    pub operation_id: String,
    pub command: String,
    pub state: State,
    pub steps: Vec<Step>,
}

/// `sha256:` and the lower-case hex of the digest.
pub fn digest(bytes: &[u8]) -> String {
    let mut text = String::from("sha256:");
    for byte in Sha256::digest(bytes) {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

pub fn path_of(root: &Path, operation_id: &str) -> PathBuf {
    root.join(DIRECTORY).join(format!("{operation_id}.json"))
}

/// Why a journal could not be used.
#[derive(Debug)]
pub enum ReadError {
    Io(std::io::Error),
    Malformed(String),
    /// Written by a version of twaco this one does not understand.
    Unsupported {
        major: u32,
        minor: u32,
    },
}

pub fn read(path: &Path) -> Result<Journal, ReadError> {
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
    serde_json::from_value(value).map_err(|why| ReadError::Malformed(why.to_string()))
}

/// Replace the journal on disk in one step and make the replacement durable.
pub fn write(root: &Path, journal: &Journal) -> std::io::Result<()> {
    let folder = root.join(DIRECTORY);
    std::fs::create_dir_all(&folder)?;
    let bytes = serde_json::to_vec_pretty(journal).expect("a journal serialises");
    crate::core::workspace::atomic_replace(&path_of(root, &journal.operation_id), &bytes)?;
    sync_directory(&folder);
    Ok(())
}

/// Make a rename in `folder` durable where the platform lets a directory be synced.
pub fn sync_directory(folder: &Path) {
    #[cfg(unix)]
    if let Ok(directory) = std::fs::File::open(folder) {
        let _ = directory.sync_all();
    }
    #[cfg(not(unix))]
    let _ = folder;
}
