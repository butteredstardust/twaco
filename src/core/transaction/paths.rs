//! Which paths a transaction may name, and the proof that none of them leaves the workspace.

use std::path::{Component, Path, PathBuf};

/// `given` as a path relative to `root`, written with `/`.
///
/// A path may be given relative or as `root` joined to one. It may not climb out (`..`), name a
/// drive or a root, or be empty: a journal that named such a path could be made to replace or
/// delete a file anywhere.
pub fn relative(root: &Path, given: &Path) -> Result<String, String> {
    let inside = if given.is_absolute() {
        given
            .strip_prefix(root)
            .map_err(|_| format!("{} is outside the workspace", given.display()))?
    } else {
        given
    };
    stored(&inside.to_string_lossy().replace('\\', "/"))
}

/// A path as a journal holds it, checked the same way.
pub fn stored(text: &str) -> Result<String, String> {
    let mut parts: Vec<&str> = Vec::new();
    for component in Path::new(text).components() {
        match component {
            Component::Normal(part) => match part.to_str() {
                Some(part) => parts.push(part),
                None => return Err(format!("{text} is not valid UTF-8")),
            },
            Component::CurDir => {}
            _ => return Err(format!("{text} is not a path inside the workspace")),
        }
    }
    if parts.is_empty() {
        return Err("an empty path".to_string());
    }
    if text.contains('\\') && !cfg!(windows) {
        return Err(format!("{text} holds a backslash"));
    }
    Ok(parts.join("/"))
}

pub fn absolute(root: &Path, relative: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    for part in relative.split('/') {
        path.push(part);
    }
    path
}

/// Whether `path` is a symbolic link or, on Windows, any reparse point (a junction, a mount).
pub fn is_link(path: &Path) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

/// Refuse a path any of whose parts, below `root`, is a link: what lies behind it is not this
/// workspace's to replace or delete. The root itself may be reached through a link.
pub fn reject_links(root: &Path, relative: &str) -> Result<(), String> {
    let mut path = root.to_path_buf();
    for part in relative.split('/') {
        path.push(part);
        if is_link(&path) {
            return Err(format!(
                "{relative} passes through the link {}",
                path.display()
            ));
        }
    }
    Ok(())
}

/// The folders that do not exist yet on the way to `relative`'s parent, shallowest first.
pub fn missing_folders(root: &Path, relative: &str) -> Vec<String> {
    let parts: Vec<&str> = relative.split('/').collect();
    let mut missing = Vec::new();
    for count in 1..parts.len() {
        let prefix = parts[..count].join("/");
        if !absolute(root, &prefix).exists() {
            missing.push(prefix);
        }
    }
    missing
}
