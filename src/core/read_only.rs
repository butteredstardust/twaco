//! Which service calls only read, so MCP `call` may run them without `dry_run: false`.
//!
//! Two lists say so: the platform services in `read_only_services.toml`, reviewed once from the
//! Javadoc, and the team's own `[call] read_only` in twaco.toml. The platform list never covers a
//! service name the repository defines or overrides: the repository's version may write.

use super::config::Solution;
use super::entity::key::ServiceTarget;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// Why a call only reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// The built-in list of platform services.
    Platform,
    /// The solution's `[call] read_only`.
    Declared,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Platform => "platform",
            Self::Declared => "twaco.toml",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BuiltIn {
    every_entity: Vec<String>,
    collections: BTreeMap<String, Vec<String>>,
    resources: BTreeMap<String, Vec<String>>,
}

fn built_in() -> &'static BuiltIn {
    static LIST: OnceLock<BuiltIn> = OnceLock::new();
    LIST.get_or_init(|| {
        toml::from_str(include_str!("read_only_services.toml"))
            .expect("the built-in read-only service list parses")
    })
}

/// Whether calling `service` on `target` only reads, and which list says so.
pub(crate) fn reason(solution: &Solution, target: &ServiceTarget, service: &str) -> Option<Reason> {
    let (collection, name) = match target {
        ServiceTarget::Thing(name) => ("Things", name.as_str()),
        ServiceTarget::Entity(key) => (key.collection(), key.name()),
    };
    if declared(&solution.call.read_only, collection, name, service) {
        return Some(Reason::Declared);
    }
    (platform(collection, name, service) && !repository_defines(solution, service))
        .then_some(Reason::Platform)
}

fn declared(entries: &[String], collection: &str, name: &str, service: &str) -> bool {
    let entity = format!("{collection}/{name}");
    entries.iter().any(|entry| {
        entry.rsplit_once('.').is_some_and(|(selector, listed)| {
            listed == service && super::glob::matches(selector, &entity)
        })
    })
}

fn platform(collection: &str, name: &str, service: &str) -> bool {
    let list = built_in();
    let has = |names: &Vec<String>| names.iter().any(|listed| listed == service);
    if collection == "Resources" {
        return list.resources.get(name).is_some_and(has);
    }
    has(&list.every_entity) || list.collections.get(collection).is_some_and(has)
}

/// Whether any repository file defines or overrides a service of this name, or might: a file
/// that cannot be read counts as one that does. Checked by name alone, not along the target's
/// inheritance, so a server-only Thing built on a repository template is covered too, and in the
/// raw XML, so every entity of a file holding several is seen.
fn repository_defines(solution: &Solution, service: &str) -> bool {
    use super::scan::{self, Kind};
    let found = super::workspace::discover(solution);
    if !found.unreadable.is_empty() {
        return true;
    }
    found.entities.iter().any(|entity| {
        let Ok(bytes) = std::fs::read(&entity.path) else {
            return true;
        };
        let Ok(tokens) = scan::tokenize(&bytes) else {
            return true;
        };
        tokens.iter().any(|token| {
            matches!(token.kind, Kind::Start | Kind::Empty)
                && matches!(
                    token.name.of(&bytes),
                    b"ServiceDefinition" | b"ServiceImplementation"
                )
                && scan::attribute(&bytes, token, "name")
                    .ok()
                    .flatten()
                    .is_some_and(|name| {
                        scan::decode_entities(&String::from_utf8_lossy(name.of(&bytes))) == service
                    })
        })
    })
}

/// Refuse a `[call] read_only` entry that names no service: `Collection/Name.Service`, the
/// name possibly a glob, split at the last dot because service names have none.
pub(crate) fn check_entry(entry: &str) -> Result<(), String> {
    let valid = entry.rsplit_once('.').is_some_and(|(selector, service)| {
        !service.is_empty()
            && service
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
            && selector
                .split_once('/')
                .is_some_and(|(collection, name)| !collection.is_empty() && !name.is_empty())
    });
    if valid {
        Ok(())
    } else {
        Err(format!(
            "[call] read_only entry {entry:?} is not Collection/Name.Service"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solution(toml: &str, files: &[(&str, &str)]) -> (tempfile::TempDir, Solution) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("twaco.toml"), toml).unwrap();
        for (path, text) in files {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let solution = Solution::load(&dir.path().join("twaco.toml")).unwrap();
        (dir, solution)
    }

    fn target(text: &str) -> ServiceTarget {
        ServiceTarget::parse(text).unwrap()
    }

    #[test]
    fn the_built_in_list_parses_and_names_only_plain_services() {
        let list = built_in();
        let names = list
            .every_entity
            .iter()
            .chain(list.collections.values().flatten())
            .chain(list.resources.values().flatten());
        for name in names {
            assert!(name.chars().all(|c| c.is_ascii_alphanumeric()), "{name:?}");
        }
        assert!(list.resources["InfoTableFunctions"].contains(&"Query".to_string()));
        assert!(!list.resources.contains_key("ContentLoaderFunctions"));
    }

    #[test]
    fn platform_reads_run_and_writes_do_not() {
        let (_dir, solution) = solution("[[project]]\nname = \"P\"\n", &[]);
        let reads = [
            ("Resources/InfoTableFunctions", "Query"),
            ("P.Orders", "GetPropertyValues"),
            ("Things/P.Table", "QueryDataTableEntries"),
            ("ThingTemplates/P.Base", "GetImplementingThings"),
        ];
        for (on, service) in reads {
            assert_eq!(
                reason(&solution, &target(on), service),
                Some(Reason::Platform),
                "{on}.{service}"
            );
        }
        let writes = [
            ("Things/P.Table", "AddDataTableEntry"),
            ("Things/P.Orders", "SetPropertyValues"),
            ("Resources/ContentLoaderFunctions", "GetJSON"),
            ("ApplicationKeys/P.Key", "GetKeyID"),
            ("Resources/SearchFunctions", "GetPropertyValues"),
        ];
        for (on, service) in writes {
            assert_eq!(
                reason(&solution, &target(on), service),
                None,
                "{on}.{service}"
            );
        }
    }

    #[test]
    fn a_service_name_the_repository_defines_is_never_a_platform_read() {
        let thing = "<Entities><Things><Thing name=\"P.Orders\" projectName=\"P\"><ThingShape>\
            <ServiceDefinitions><ServiceDefinition name=\"GetPropertyValues\"/></ServiceDefinitions>\
            </ThingShape></Thing></Things></Entities>";
        let (_dir, solution) = solution(
            "[[project]]\nname = \"P\"\n",
            &[("Things/P.Orders.xml", thing)],
        );
        for on in ["Things/P.Orders", "Things/P.Elsewhere"] {
            assert_eq!(
                reason(&solution, &target(on), "GetPropertyValues"),
                None,
                "{on}"
            );
        }
        assert_eq!(
            reason(&solution, &target("Things/P.Orders"), "GetPropertyTime"),
            Some(Reason::Platform)
        );
    }

    #[test]
    fn a_second_entity_in_a_file_and_an_unreadable_file_also_count_as_defining() {
        let two = "<Entities><Things><Thing name=\"P.First\" projectName=\"P\"/>\
            <Thing name=\"P.Second\" projectName=\"P\"><ThingShape><ServiceDefinitions>\
            <ServiceDefinition name=\"GetPropertyValues\"/></ServiceDefinitions></ThingShape>\
            </Thing></Things></Entities>";
        let (_first, several) = solution(
            "[[project]]\nname = \"P\"\n",
            &[("Things/P.First.xml", two)],
        );
        assert_eq!(
            reason(&several, &target("Things/P.X"), "GetPropertyValues"),
            None
        );
        let (_second, broken) = solution(
            "[[project]]\nname = \"P\"\n",
            &[("Things/P.Broken.xml", "<Entities><Things><Thing name=")],
        );
        assert_eq!(
            reason(&broken, &target("Things/P.X"), "GetPropertyTime"),
            None
        );
    }

    #[test]
    fn the_team_list_names_services_exactly_or_by_glob() {
        let (_dir, solution) = solution(
            "[[project]]\nname = \"P\"\n[call]\nread_only = [\"Things/P.Orders.GetOrders\", \"Things/P.*.GetSummary\"]\n",
            &[],
        );
        assert_eq!(
            reason(&solution, &target("Things/P.Orders"), "GetOrders"),
            Some(Reason::Declared)
        );
        assert_eq!(
            reason(&solution, &target("P.Lines"), "GetSummary"),
            Some(Reason::Declared)
        );
        assert_eq!(
            reason(&solution, &target("Things/P.Lines"), "GetOrders"),
            None
        );
        assert_eq!(
            reason(&solution, &target("Things/Q.Lines"), "GetSummary"),
            None
        );
    }

    #[test]
    fn a_team_entry_without_a_service_is_refused_when_the_config_loads() {
        for bad in [
            "P.Orders.GetOrders",
            "Things/.Get",
            "Things/P.Orders.",
            "Things/P.Orders.Get-Orders",
        ] {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(
                dir.path().join("twaco.toml"),
                format!("[[project]]\nname = \"P\"\n[call]\nread_only = [{bad:?}]\n"),
            )
            .unwrap();
            let error = Solution::load(&dir.path().join("twaco.toml"))
                .unwrap_err()
                .to_string();
            assert!(error.contains("Collection/Name.Service"), "{bad}: {error}");
        }
    }
}
