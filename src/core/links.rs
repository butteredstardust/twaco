//! Links and Windows reparse points that must not be traversed for a workspace write.

use std::path::Path;

/// Whether `path` is a symbolic link or, on Windows, any reparse point (a junction, a mount).
pub(crate) fn is_link(path: &Path) -> bool {
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
pub(crate) fn reject_links(root: &Path, relative: &str) -> Result<(), String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn directory_link(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
        #[cfg(windows)]
        {
            std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .output()
                .is_ok_and(|output| output.status.success())
        }
    }

    #[test]
    fn identifies_links_and_leaves_ordinary_or_missing_paths_alone() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let file = root.join("file");
        let folder = root.join("folder");
        let target = root.join("target");
        let link = root.join("link");
        std::fs::write(&file, b"x").unwrap();
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        if !directory_link(&target, &link) {
            return;
        }

        assert!(!is_link(&file));
        assert!(!is_link(&folder));
        assert!(is_link(&link));
        assert!(!is_link(&root.join("missing")));
    }

    #[test]
    fn rejects_a_link_component_but_allows_file_directory_and_missing_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("file"), b"x").unwrap();
        std::fs::create_dir_all(root.join("folder")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        let link = root.join("link");
        if !directory_link(&root.join("target"), &link) {
            return;
        }

        assert!(reject_links(root, "file").is_ok());
        assert!(reject_links(root, "folder/child").is_ok());
        assert!(reject_links(root, "missing/child").is_ok());
        let error = reject_links(root, "link/child").unwrap_err();
        assert!(error.contains(&link.display().to_string()), "{error}");
    }
}
