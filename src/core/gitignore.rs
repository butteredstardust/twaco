//! The `.gitignore` lines a solution needs for what twaco keeps beside the code and must not
//! commit: server profiles (credentials), backups (a server's own copy of an entity, which can
//! hold secrets such as a database password), the crash journal, locks and generated declarations.
//!
//! `twaco init` adds what is missing; `twaco doctor` says what is. A file already covering a line
//! (the line itself, its folder, or all of `.twaco`) counts, and an existing file is only ever
//! appended to, in its own line endings.

use std::io::Write;
use std::path::Path;

/// What a solution should not commit.
pub const ENTRIES: &[&str] = &[
    ".twaco/profiles/",
    ".twaco/lock",
    ".twaco/lock.holder",
    ".twaco/backups/",
    ".twaco/transactions/",
    ".twaco/types/",
    ".twaco/platform.json",
    "**/services/*/jsconfig.json",
    "**/services/*/twaco-globals.d.ts",
];

const HEADING: &str = "# twaco: local state that must not be committed";

fn normalised(line: &str) -> &str {
    line.trim().trim_start_matches('/').trim_end_matches('/')
}

/// Whether the working tree of `root` is in a git repository: a `.git` here or in a folder above.
pub fn in_git_work_tree(root: &Path) -> bool {
    root.ancestors().any(|folder| folder.join(".git").exists())
}

fn covered(entry: &str, lines: &[&str]) -> bool {
    let wanted = normalised(entry);
    let whole_folder = wanted.starts_with(".twaco/");
    lines.iter().any(|line| {
        *line == wanted
            || (whole_folder && matches!(*line, ".twaco" | ".twaco/**" | ".twaco/*"))
            || *line == format!("{wanted}/**")
    })
}

/// The entries the solution's `.gitignore` does not cover yet; all of them without a file.
pub fn missing(root: &Path) -> Vec<&'static str> {
    let text = std::fs::read_to_string(root.join(".gitignore")).unwrap_or_default();
    let lines: Vec<&str> = text
        .lines()
        .map(normalised)
        .filter(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with('!'))
        .collect();
    ENTRIES
        .iter()
        .copied()
        .filter(|entry| !covered(entry, &lines))
        .collect()
}

/// Add the missing entries as one block at the end of the file, creating it if it is not there.
/// Returns what was added.
pub fn add_missing(root: &Path) -> std::io::Result<Vec<&'static str>> {
    let missing = missing(root);
    if missing.is_empty() {
        return Ok(missing);
    }
    let path = root.join(".gitignore");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let eol = super::workspace::line_ending(&existing);
    let mut block = String::new();
    if !existing.is_empty() {
        if !existing.ends_with('\n') {
            block.push_str(eol);
        }
        block.push_str(eol);
    }
    block.push_str(HEADING);
    block.push_str(eol);
    for entry in &missing {
        block.push_str(entry);
        block.push_str(eol);
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?
        .write_all(block.as_bytes())?;
    Ok(missing)
}

/// The folders whose files can hold a secret: profiles hold credentials; backups and journal
/// backups hold a server's copy of an entity, a database Thing's password included.
pub const SECRET_FOLDERS: &[&str] = &[".twaco/profiles", ".twaco/backups", ".twaco/transactions"];

/// The files under [`SECRET_FOLDERS`] that git already tracks, relative to `root`. An ignore line
/// does not untrack a file that was added before it, so `.gitignore` alone cannot answer this.
/// Asks `git ls-files`, which reads the index and nothing else; an error says why git could not
/// be asked.
pub fn tracked_secrets(root: &Path) -> Result<Vec<String>, String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z", "--"])
        .args(SECRET_FOLDERS)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|error| format!("could not run git: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git ls-files failed: {}", stderr.trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder() -> std::path::PathBuf {
        let nonce = crate::test_nonce();
        let root =
            std::env::temp_dir().join(format!("twaco-gitignore-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn without_a_file_everything_is_missing_and_adding_creates_it() {
        let root = folder();
        assert_eq!(missing(&root), ENTRIES);
        assert_eq!(add_missing(&root).unwrap(), ENTRIES);
        let text = std::fs::read_to_string(root.join(".gitignore")).unwrap();
        assert!(text.starts_with(HEADING), "{text}");
        for entry in ENTRIES {
            assert!(text.lines().any(|line| line == *entry), "{entry}");
        }
        assert!(missing(&root).is_empty());
        assert!(add_missing(&root).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_line_its_folder_or_all_of_the_twaco_folder_covers_an_entry() {
        let root = folder();
        std::fs::write(
            root.join(".gitignore"),
            "/.twaco/backups\n.twaco/transactions/**\n# .twaco/lock\n",
        )
        .unwrap();
        let missing = missing(&root);
        assert!(!missing.contains(&".twaco/backups/"));
        assert!(!missing.contains(&".twaco/transactions/"));
        assert!(missing.contains(&".twaco/lock"), "a comment covers nothing");
        // The whole folder is not a line twaco wrote, but it covers every `.twaco` entry.
        for line in [".twaco/", ".twaco", "/.twaco/", ".twaco/*", ".twaco/**"] {
            std::fs::write(root.join(".gitignore"), format!("{line}\n")).unwrap();
            assert!(
                !super::missing(&root)
                    .iter()
                    .any(|entry| entry.starts_with(".twaco/")),
                "{line}"
            );
        }
        assert!(super::missing(&root).contains(&"**/services/*/jsconfig.json"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_existing_file_is_appended_to_in_its_own_line_endings() {
        let root = folder();
        std::fs::write(root.join(".gitignore"), "target/\r\n.twaco/profiles/").unwrap();
        let added = add_missing(&root).unwrap();
        assert!(!added.contains(&".twaco/profiles/"));
        let text = std::fs::read(root.join(".gitignore")).unwrap();
        let text = String::from_utf8(text).unwrap();
        assert!(
            text.starts_with("target/\r\n.twaco/profiles/\r\n\r\n# twaco"),
            "{text:?}"
        );
        assert_eq!(
            text.matches('\n').count(),
            text.matches("\r\n").count(),
            "no bare line feed was added: {text:?}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_work_tree_is_found_from_a_folder_below_the_git_folder() {
        let root = folder();
        let below = root.join("a/b");
        std::fs::create_dir_all(&below).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        assert!(in_git_work_tree(&below));
        let _ = std::fs::remove_dir_all(root);
    }

    fn git(root: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .stdout(std::process::Stdio::null())
            .status()
            .expect("git is on PATH");
        assert!(status.success(), "git {args:?}");
    }

    #[test]
    fn a_tracked_profile_or_backup_is_found_even_when_ignored_since() {
        let root = folder();
        git(&root, &["init", "-q"]);
        for path in [
            ".twaco/profiles/default.toml",
            ".twaco/backups/Things/A.T.xml",
            ".twaco/types/twaco.d.ts",
            "Things/A.T.xml",
        ] {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "x").unwrap();
        }
        assert_eq!(tracked_secrets(&root).unwrap(), Vec::<String>::new());
        git(&root, &["add", "-A"]);
        add_missing(&root).unwrap();
        assert_eq!(
            tracked_secrets(&root).unwrap(),
            [
                ".twaco/backups/Things/A.T.xml",
                ".twaco/profiles/default.toml"
            ],
            "the ignore lines added afterwards do not untrack them"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn outside_a_repository_git_says_why() {
        let root = folder();
        // A temp folder inside a repository would answer; a test machine's should not be.
        if !in_git_work_tree(&root) {
            let error = tracked_secrets(&root).unwrap_err();
            assert!(error.starts_with("git ls-files failed"), "{error}");
        }
        let _ = std::fs::remove_dir_all(root);
    }
}
