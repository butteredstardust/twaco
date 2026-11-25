//! Which service calls only read, so `call` (CLI) and MCP `call` run them without `--apply` or
//! `dry_run: false`.
//!
//! In order: the team's `[call] read` / `read_only` in twaco.toml; for a `Resources/*` entity only
//! the built-in list (the name prefix never applies there: ContentLoaderFunctions, EntityServices
//! and the like write under Get-like names too); the platform's own category (READ or QUERY, from
//! the platform cache); the platform services in `read_only_services.toml`, reviewed once from the
//! Javadoc, which never cover a name the repository defines or overrides; and last the name
//! (Get, Query, Is, Has, Search, Find followed by a capital) when nothing else is known. Anything
//! else is a write.

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
    /// The solution's `[call] read` or `read_only`.
    Declared,
    /// The platform cache says the service's category is READ or QUERY.
    Category,
    /// The name starts with Get, Query, Is, Has, Search or Find.
    Name,
    /// The caller passed `--read`.
    Flag,
}

/// Whether a call reads (and why) or writes (and what to say about it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Class {
    Read(Reason),
    Write(String),
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Platform => "platform",
            Self::Declared => "twaco.toml",
            Self::Category => "category",
            Self::Name => "name",
            Self::Flag => "--read",
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

/// The name prefixes of a read when no metadata says otherwise.
const READ_PREFIXES: [&str; 6] = ["Get", "Query", "Is", "Has", "Search", "Find"];

/// Reads by name that still return something secret or reach other hosts, so a prefix never
/// vouches for them (the built-in list leaves them out for the same reason).
fn never_by_name(service: &str) -> bool {
    service == "GetKeyID" || service.starts_with("GetConfigurationTable")
}

fn by_name(service: &str) -> bool {
    !never_by_name(service)
        && READ_PREFIXES.iter().any(|prefix| {
            service.strip_prefix(prefix).is_some_and(|rest| {
                rest.chars()
                    .next()
                    .is_none_or(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            })
        })
}

/// The category the platform cache holds for a platform service, if it holds one.
fn category(solution: &Solution, collection: &str, name: &str, service: &str) -> Option<String> {
    let platform = super::types::platform::cached(solution)?;
    let meta = match collection {
        "Things" => platform.things.get(name),
        "ThingTemplates" => platform.templates.get(name),
        "ThingShapes" => platform.shapes.get(name),
        "Resources" => platform.resources.get(name),
        _ => None,
    }?;
    let found = &meta.services.get(service)?.category;
    (!found.is_empty()).then(|| found.clone())
}

/// Whether calling `service` on `target` only reads, and why; otherwise the sentence that says
/// it is a write and how to say it only reads.
pub(crate) fn classify(solution: &Solution, target: &ServiceTarget, service: &str) -> Class {
    let (collection, name) = match target {
        ServiceTarget::Thing(name) => ("Things", name.as_str()),
        ServiceTarget::Entity(key) => (key.collection(), key.name()),
    };
    let declared_here = declared(&solution.call.read_only, collection, name, service)
        || declared(&solution.call.read, collection, name, service);
    if declared_here {
        return Class::Read(Reason::Declared);
    }
    let add = "add it to [call] read in twaco.toml or pass --read if it only reads";
    if collection == "Resources" {
        return if platform(collection, name, service) && !repository_defines(solution, service) {
            Class::Read(Reason::Platform)
        } else {
            Class::Write(format!(
                "write: {service} (a Resources service is a write unless it is on the built-in read list; {add})"
            ))
        };
    }
    let known = category(solution, collection, name, service);
    if let Some(found) = &known {
        // Categories only ever add reads: the platform's own category names are not verified,
        // so one that is not a read category leaves the name rule in charge.
        let reads = ["READ", "QUERY", "QUERIES"]
            .iter()
            .any(|read| found.eq_ignore_ascii_case(read));
        if reads && !repository_defines(solution, service) {
            return Class::Read(Reason::Category);
        }
    }
    if platform(collection, name, service) && !repository_defines(solution, service) {
        return Class::Read(Reason::Platform);
    }
    // An explicit WRITE category outranks a reading name; any other category leaves it to the name.
    let says_write = known
        .as_deref()
        .is_some_and(|found| found.eq_ignore_ascii_case("WRITE"));
    if !says_write && by_name(service) {
        return Class::Read(Reason::Name);
    }
    let why = match known {
        Some(found) => format!("category {found}, not READ; name not Get/Query/Is/Has/Search/Find"),
        None => "no READ category; name not Get/Query/Is/Has/Search/Find".to_string(),
    };
    Class::Write(format!("write: {service} ({why}; {add})"))
}

/// Whether calling `service` on `target` only reads, and which rule says so.
#[cfg(test)]
pub(crate) fn reason(solution: &Solution, target: &ServiceTarget, service: &str) -> Option<Reason> {
    match classify(solution, target, service) {
        Class::Read(why) => Some(why),
        Class::Write(_) => None,
    }
}

fn declared(entries: &[String], collection: &str, name: &str, service: &str) -> bool {
    let entity = format!("{collection}/{name}");
    entries.iter().any(|entry| {
        entry.rsplit_once('.').is_some_and(|(selector, listed)| {
            listed == service
                && (super::glob::matches(selector, &entity) || super::glob::matches(selector, name))
        })
    })
}

fn platform(collection: &str, name: &str, service: &str) -> bool {
    let list = built_in();
    let has = |names: &Vec<String>| names.iter().any(|listed| listed == service);
    if collection == "Resources" {
        return list.resources.get(name).is_some_and(has) || has(&list.every_entity);
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

/// Refuse a `[call] read` entry that names no service: `Entity.Service` or
/// `Collection/Entity.Service`, the entity possibly a glob (`Acme.*.GetSummary`).
pub(crate) fn check_read_entry(entry: &str) -> Result<(), String> {
    let valid = entry.rsplit_once('.').is_some_and(|(selector, service)| {
        !selector.is_empty()
            && !service.is_empty()
            && service
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
    });
    if valid {
        Ok(())
    } else {
        Err(format!(
            "[call] read entry {entry:?} is not Entity.Service or Collection/Entity.Service"
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
        // The platform list is not used, though the name still reads.
        for on in ["Things/P.Orders", "Things/P.Elsewhere"] {
            assert_eq!(
                reason(&solution, &target(on), "GetPropertyValues"),
                Some(Reason::Name),
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
            Some(Reason::Name)
        );
        let (_second, broken) = solution(
            "[[project]]\nname = \"P\"\n",
            &[("Things/P.Broken.xml", "<Entities><Things><Thing name=")],
        );
        assert_eq!(
            reason(&broken, &target("Things/P.X"), "GetPropertyTime"),
            Some(Reason::Name)
        );
    }

    #[test]
    fn a_read_by_name_needs_a_capital_after_the_prefix_and_never_covers_secrets() {
        let (_dir, solution) = solution(
            "[[project]]
name = \"P\"
",
            &[],
        );
        let on = target("Things/P.Orders");
        for read in [
            "GetOrders",
            "QueryRows",
            "IsReady",
            "HasStock",
            "SearchAll",
            "Find",
        ] {
            assert_eq!(reason(&solution, &on, read), Some(Reason::Name), "{read}");
        }
        for write in [
            "Getter",
            "Issue",
            "Delete",
            "Settle",
            "GetKeyID",
            "GetConfigurationTableRows",
        ] {
            assert_eq!(reason(&solution, &on, write), None, "{write}");
        }
    }

    #[test]
    fn a_resources_service_reads_only_from_the_built_in_list_or_the_team_list() {
        let (_dir, solution) = solution(
            "[[project]]
name = \"P\"
[call]
read = [\"EntityServices.Odd\"]
",
            &[],
        );
        let on = target("Resources/EntityServices");
        assert_eq!(
            reason(&solution, &on, "GetEntityList"),
            Some(Reason::Platform)
        );
        assert_eq!(
            reason(&solution, &on, "GetIncomingDependencies"),
            Some(Reason::Platform)
        );
        assert_eq!(reason(&solution, &on, "Odd"), Some(Reason::Declared));
        // Get-prefixed, but not on the list: a write.
        assert_eq!(reason(&solution, &on, "GetSomethingNew"), None);
        let Class::Write(why) = classify(&solution, &on, "DeleteDataTagVocabulary") else {
            panic!("a write")
        };
        assert!(why.contains("[call] read"), "{why}");
    }

    #[test]
    fn the_platform_category_makes_an_oddly_named_service_a_read() {
        let cache = r#"{"things":{"P.Orders":{"services":{
            "Compute":{"category":"READ","result":{"baseType":"NOTHING"}},
            "Wipe":{"category":"WRITE","result":{"baseType":"NOTHING"}},
            "GetAndWipe":{"category":"WRITE","result":{"baseType":"NOTHING"}}}}}}"#;
        let (_dir, solution) = solution(
            "[[project]]
name = \"P\"
",
            &[(".twaco/platform.json", cache)],
        );
        let on = target("Things/P.Orders");
        assert_eq!(reason(&solution, &on, "Compute"), Some(Reason::Category));
        assert_eq!(reason(&solution, &on, "Wipe"), None);
        // Metadata that says WRITE outranks a reading name.
        let Class::Write(why) = classify(&solution, &on, "GetAndWipe") else {
            panic!("a write")
        };
        assert!(why.contains("category WRITE"), "{why}");
    }

    #[test]
    fn an_unknown_category_leaves_a_reading_name_a_read() {
        // The platform's own category names are not verified; only READ/QUERY add a read and only
        // WRITE takes one away.
        let cache = r#"{"things":{"P.Orders":{"services":{
            "GetOrderTotals":{"category":"Properties","result":{"baseType":"INFOTABLE"}},
            "Reset":{"category":"Properties","result":{"baseType":"NOTHING"}}}}}}"#;
        let (_dir, solution) = solution(
            "[[project]]
name = \"P\"
",
            &[(".twaco/platform.json", cache)],
        );
        let on = target("Things/P.Orders");
        assert_eq!(reason(&solution, &on, "GetOrderTotals"), Some(Reason::Name));
        assert_eq!(reason(&solution, &on, "Reset"), None);
    }

    #[test]
    fn read_entries_take_a_bare_entity_name_or_a_glob() {
        let (_dir, solution) = solution(
            "[[project]]
name = \"P\"
[call]
read = [\"P.Orders.Odd\", \"P.*.Strange\"]
",
            &[],
        );
        let reads = |on: &str, service: &str| reason(&solution, &target(on), service);
        assert_eq!(reads("P.Orders", "Odd"), Some(Reason::Declared));
        assert_eq!(reads("Things/P.Orders", "Odd"), Some(Reason::Declared));
        assert_eq!(reads("P.Lines", "Strange"), Some(Reason::Declared));
        assert_eq!(reads("P.Lines", "Odd"), None);
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
        assert_ne!(
            reason(&solution, &target("Things/P.Lines"), "GetOrders"),
            Some(Reason::Declared)
        );
        assert_ne!(
            reason(&solution, &target("Things/Q.Lines"), "GetSummary"),
            Some(Reason::Declared)
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
