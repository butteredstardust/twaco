//! `ext intellij`: put twaco's IntelliJ IDEA live templates (the ThingWorx annotations) and
//! External Tools (`ext check`, `ext add`, `ext deploy`) into an IntelliJ configuration folder,
//! which IntelliJ reads when it starts.

use serde::Serialize;
use std::path::{Path, PathBuf};

const FILES: [(&str, &str); 2] = [
    (
        "templates/ThingWorx.xml",
        include_str!("../../editors/intellij/ThingWorx.xml"),
    ),
    (
        "tools/twaco.xml",
        include_str!("../../editors/intellij/twaco.xml"),
    ),
];

#[derive(Debug, Serialize)]
pub struct Install {
    /// The IntelliJ configuration folder.
    pub config: String,
    /// Each file with `create`, `replace` or `unchanged`.
    pub files: Vec<(String, &'static str)>,
    #[serde(skip)]
    root: PathBuf,
}

impl Install {
    /// Write the files that are not already as twaco has them.
    pub fn apply(&self) -> Result<(), String> {
        for ((path, state), (_, text)) in self.files.iter().zip(FILES) {
            if *state == "unchanged" {
                continue;
            }
            let target = self.root.join(path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
            }
            std::fs::write(&target, text)
                .map_err(|error| format!("cannot write {}: {error}", target.display()))?;
        }
        Ok(())
    }
}

/// What installing into `config` would do.
pub fn plan(config: &Path) -> Install {
    let files = FILES
        .iter()
        .map(|(path, text)| {
            let state = match std::fs::read_to_string(config.join(path)) {
                Ok(existing) if existing == *text => "unchanged",
                Ok(_) => "replace",
                Err(_) => "create",
            };
            (path.to_string(), state)
        })
        .collect();
    Install {
        config: config.display().to_string(),
        files,
        root: config.to_path_buf(),
    }
}

/// The newest IntelliJ IDEA configuration folder (Ultimate or Community) under JetBrains'
/// folder for this user.
pub fn newest_config() -> Result<PathBuf, String> {
    let base = jetbrains_folder().ok_or("cannot tell where JetBrains keeps its configuration")?;
    newest_in(&base).ok_or_else(|| {
        format!(
            "no IntelliJ IDEA configuration under {}: start IntelliJ once, or pass --dir",
            base.display()
        )
    })
}

fn jetbrains_folder() -> Option<PathBuf> {
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(|appdata| PathBuf::from(appdata).join("JetBrains"))
    } else if cfg!(target_os = "macos") {
        home().map(|home| home.join("Library/Application Support/JetBrains"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home().map(|home| home.join(".config")))
            .map(|config| config.join("JetBrains"))
    }
}

/// `IntelliJIdea2026.1` or `IdeaIC2024.1`: the one with the highest version.
fn newest_in(base: &Path) -> Option<PathBuf> {
    std::fs::read_dir(base)
        .ok()?
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let version = ["IntelliJIdea", "IdeaIC"]
                .iter()
                .find_map(|prefix| name.strip_prefix(prefix))?;
            let parts: Vec<u32> = version
                .split('.')
                .map(|part| part.parse().ok())
                .collect::<Option<_>>()?;
            Some((parts, entry.path()))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, path)| path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_idea_folder_is_chosen_and_files_are_written_once() {
        let temporary = tempfile::tempdir().unwrap();
        for name in [
            "IdeaIC2024.1",
            "IntelliJIdea2026.1",
            "IntelliJIdea2025.3",
            "DataGrip2030.1",
        ] {
            std::fs::create_dir(temporary.path().join(name)).unwrap();
        }
        let config = newest_in(temporary.path()).unwrap();
        assert!(config.ends_with("IntelliJIdea2026.1"));

        let install = plan(&config);
        assert_eq!(
            install.files,
            [
                ("templates/ThingWorx.xml".to_string(), "create"),
                ("tools/twaco.xml".to_string(), "create")
            ]
        );
        install.apply().unwrap();
        std::fs::write(config.join("tools/twaco.xml"), "<toolSet/>").unwrap();
        let again = plan(&config);
        assert_eq!(again.files[0].1, "unchanged");
        assert_eq!(again.files[1].1, "replace");
    }

    /// Each template's text with its variables at their defaults.
    fn expanded() -> std::collections::BTreeMap<String, String> {
        use super::super::scan::{self, Kind};
        let text = FILES[0].1.as_bytes();
        let tokens = scan::tokenize(text).unwrap();
        let attribute = |token: &scan::Token, name: &str| {
            scan::attribute(text, token, name)
                .unwrap()
                .map(|span| scan::decode_entities(&String::from_utf8_lossy(span.of(text))))
                .unwrap_or_default()
        };
        let mut out = std::collections::BTreeMap::new();
        let mut current: Option<(String, String)> = None;
        for token in &tokens {
            if !matches!(token.kind, Kind::Start | Kind::Empty) {
                continue;
            }
            match token.name.of(text) {
                b"template" => {
                    if let Some((name, value)) = current.take() {
                        out.insert(name, value);
                    }
                    current = Some((attribute(token, "name"), attribute(token, "value")));
                }
                b"variable" => {
                    let (_, value) = current.as_mut().unwrap();
                    let default = attribute(token, "defaultValue");
                    let default = default.trim_matches('"');
                    let name = attribute(token, "name");
                    let default = if name == "CLASS" { "Pump" } else { default };
                    *value = value.replace(&format!("${name}$"), default);
                }
                _ => {}
            }
        }
        if let Some((name, value)) = current {
            out.insert(name, value);
        }
        out.into_iter()
            .map(|(name, value)| (name, value.replace("$END$", "")))
            .collect()
    }

    #[test]
    fn what_the_templates_write_is_what_the_extension_reader_reads() {
        let templates = expanded();
        let service = templates["twxservice"].replace(
            "DoSomething() throws Exception {\n    \n}",
            &format!(
                "DoSomething({}) throws Exception {{ return value; }}",
                templates["twxparam"]
            ),
        );
        let java = format!(
            "package com.acme;\n{}\n{}\npublic class Pump extends Thing {{\n{}\n{}\n}}\n",
            templates["twxprops"], templates["twxevents"], templates["twxlog"], service
        );
        let parsed = super::super::extension_source::parse_java(&java).unwrap();
        let names: Vec<&str> = parsed.members.iter().map(|m| m.name()).collect();
        assert_eq!(names, ["DoSomething", "Status"], "{java}");
        let super::super::types::Member::Service(service) = &parsed.members[0] else {
            panic!("{java}")
        };
        assert_eq!(service.parameters[0].name, "value");
        assert_eq!(service.result.base_type, "STRING");
        assert_eq!(parsed.events[0].0, "Alarm");
    }

    #[test]
    fn the_shipped_files_are_well_formed_and_name_what_twaco_reads() {
        for (path, text) in FILES {
            super::super::scan::tokenize(text.as_bytes())
                .unwrap_or_else(|error| panic!("{path}: {error}"));
        }
        // Every annotation a template writes is one the extension reader knows.
        let templates = FILES[0].1;
        for annotation in [
            "ThingworxServiceDefinition",
            "ThingworxServiceResult",
            "ThingworxServiceParameter",
            "ThingworxPropertyDefinitions",
            "ThingworxEventDefinitions",
        ] {
            assert!(templates.contains(annotation), "{annotation}");
        }
    }
}
