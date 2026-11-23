//! The repository is public: platform kit entity names (their projects and mashups) must not appear in
//! committed files. Components and docs name a pattern in plain words instead ("AdminPage", "Compare").

use std::path::Path;

/// Kit project and entity prefixes, assembled so this file does not match itself.
fn forbidden() -> Vec<String> {
    vec![
        ["PTC", "SC."].concat(),
        ["IOTCommon", "MU"].concat(),
        ["ThingWorx", "DesignCentral"].concat(),
        ["Thingworx", "DesignCentral"].concat(),
        ["TWXUI", "Catalogue"].concat(),
        ["WDK", "_"].concat(),
    ]
}

fn scan(path: &Path, words: &[String], hits: &mut Vec<String>) {
    if path.is_dir() {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if matches!(name, "target" | ".git" | "node_modules") {
            return;
        }
        for entry in std::fs::read_dir(path).unwrap() {
            scan(&entry.unwrap().path(), words, hits);
        }
        return;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    for (n, line) in text.lines().enumerate() {
        if let Some(word) = words.iter().find(|w| line.contains(w.as_str())) {
            hits.push(format!("{}:{}: {word}", path.display(), n + 1));
        }
    }
}

#[test]
fn committed_files_name_no_platform_kit_entities() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let words = forbidden();
    let mut hits = vec![];
    for part in [
        "src",
        "tests",
        "documentation",
        "knowledge",
        "README.md",
        "AGENTS.md",
        "CHANGELOG.md",
    ] {
        scan(&root.join(part), &words, &mut hits);
    }
    assert!(
        hits.is_empty(),
        "use a plain pattern name instead of kit entity names:\n{}",
        hits.join("\n")
    );
}
