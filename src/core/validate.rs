//! Whether the entity files in a solution are internally consistent.
//!
//! These are the checks that catch a repository which looks fine and imports wrong: a file whose
//! name disagrees with the entity inside it, two files claiming the same entity, a service half
//! declared, a mashup whose JSON payload will not parse.

use super::config::Solution;
use super::scan::{self, Kind};
use super::types::{platform::Platform, Model};
use super::workspace::{self, EntityFile};
use std::cell::OnceCell;
use std::collections::{BTreeMap, BTreeSet};

/// One inconsistency, with the file it is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Problem {
    pub(crate) file: String,
    pub rule: &'static str,
    pub message: String,
}

/// Check every entity in the solution.
#[cfg(test)]
pub(crate) fn check(solution: &Solution) -> Vec<Problem> {
    let found = workspace::discover(solution);
    check_in(solution, &found)
}

/// [`check`] over a scan already made.
pub(crate) fn check_in(solution: &Solution, found: &workspace::Discovery) -> Vec<Problem> {
    let mut problems = Vec::new();

    // A name claimed by two files: one of them loses on import, silently.
    let mut seen: BTreeMap<&str, &EntityFile> = BTreeMap::new();
    for entity in &found.entities {
        if let Some(first) = seen.get(entity.info.name.as_str()) {
            problems.push(Problem {
                file: relative(solution, entity),
                rule: "duplicate-entity",
                message: format!(
                    "{} is already declared by {}",
                    entity.info.name,
                    relative(solution, first)
                ),
            });
        } else {
            seen.insert(entity.info.name.as_str(), entity);
        }
    }

    let vocabularies = vocabularies(found);
    let ancestry = Ancestry {
        solution,
        loaded: OnceCell::new(),
    };
    for entity in &found.entities {
        problems.extend(check_one(solution, entity, &vocabularies, &ancestry));
    }

    for problem in &found.unreadable {
        problems.push(Problem {
            // `<path>: <why>`; the first colon of a Windows path is its drive letter.
            file: {
                let file = problem
                    .split_once(": ")
                    .map_or(problem.as_str(), |(file, _)| file);
                let path = std::path::Path::new(file);
                path.strip_prefix(&solution.root)
                    .unwrap_or(path)
                    .display()
                    .to_string()
                    .replace('\\', "/")
            },
            rule: "unreadable",
            message: problem.clone(),
        });
    }
    problems
}

/// Whether an ancestor defines a service an entity implements with no definition of its own: a
/// template or shape of the solution or of a configured extension, or one the platform cache
/// holds. Loaded the first time a file needs it; only a definition found counts, so an
/// incomplete chain or a missing cache leaves the finding standing.
struct Ancestry<'a> {
    solution: &'a Solution,
    loaded: OnceCell<(Model, Option<Platform>)>,
}

impl Ancestry<'_> {
    fn defines(&self, collection: &str, name: &str, service: &str) -> bool {
        let (model, platform) = self.loaded.get_or_init(|| {
            (
                super::types::load_model(self.solution).0,
                super::types::platform::cached(self.solution),
            )
        });
        let Some(entity) = model
            .entities
            .iter()
            .find(|entity| entity.collection == collection && entity.name == name)
        else {
            return false;
        };
        super::catalog::lookup_service(entity, &model.entities, platform.as_ref(), service)
            .service
            .is_some()
    }
}

fn check_one(
    solution: &Solution,
    entity: &EntityFile,
    vocabularies: &Vocabularies,
    ancestry: &Ancestry,
) -> Vec<Problem> {
    let mut problems = Vec::new();
    let file = relative(solution, entity);

    // The file name is how every other tool addresses the entity, including the bundler.
    let stem = entity
        .path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    if stem != entity.info.name {
        problems.push(Problem {
            file: file.clone(),
            rule: "name-mismatch",
            message: format!(
                "the file is named {stem} but the entity inside it is {}",
                entity.info.name
            ),
        });
    }

    if entity.info.name.is_empty() {
        problems.push(Problem {
            file: file.clone(),
            rule: "unnamed",
            message: "the entity element has no name attribute".to_string(),
        });
    }

    // An entity with no project imports into the platform's default one, where nothing looks
    // for it and no bundle carries it.
    if entity.info.project.is_empty() {
        problems.push(Problem {
            file: file.clone(),
            rule: "no-project",
            message: format!(
                "declares no projectName; it would import into the platform default rather \
                 than into {}",
                entity.found_under
            ),
        });
    }

    if entity.is_misfiled() {
        problems.push(Problem {
            file: file.clone(),
            rule: "misfiled",
            message: format!(
                "declares projectName {} but sits under {}; it would import into the wrong project",
                entity.info.project, entity.found_under
            ),
        });
    }

    let Ok(src) = std::fs::read(&entity.path) else {
        return problems;
    };

    problems.extend(check_services(
        &file,
        &entity.info.name,
        &src,
        &solution.validate.inherited_overrides,
        &|service| ancestry.defines(&entity.info.collection, &entity.info.name, service),
    ));
    problems.extend(check_member_names(&file, &src));
    if entity.info.collection == "Mashups" {
        problems.extend(check_mashup(&file, &src));
    }
    problems.extend(check_tags(&file, &src, vocabularies));
    problems
}

/// The importer shares one namespace between local properties, services and events.
fn check_member_names(file: &str, src: &[u8]) -> Vec<Problem> {
    let Ok(tokens) = scan::tokenize(src) else {
        return Vec::new();
    };
    let mut names = BTreeMap::new();
    let mut problems = Vec::new();
    for (tag, kind) in [
        ("PropertyDefinition", "property"),
        ("ServiceDefinition", "service"),
        ("EventDefinition", "event"),
    ] {
        for token in tokens.iter().filter(|token| {
            matches!(token.kind, Kind::Start | Kind::Empty) && token.name.of(src) == tag.as_bytes()
        }) {
            let Some(name) = scan::attribute(src, token, "name")
                .ok()
                .flatten()
                .map(|span| scan::decode_entities(&String::from_utf8_lossy(span.of(src))))
            else {
                continue;
            };
            if let Some(previous) = names.insert(name.clone(), kind) {
                problems.push(Problem {
                    file: file.to_string(),
                    rule: "member-name-clash",
                    message: format!(
                        "{kind} {name} has the same name as a {previous}; rename one before import"
                    ),
                });
            }
        }
    }
    problems
}

/// The solution's own model tag vocabularies: the terms of each, or none for a dynamic one,
/// which takes any term.
type Vocabularies = BTreeMap<String, Option<BTreeSet<String>>>;

fn vocabularies(found: &workspace::Discovery) -> Vocabularies {
    let mut out = Vocabularies::new();
    for file in found
        .entities
        .iter()
        .filter(|file| file.info.collection == "ModelTags")
    {
        let Ok(src) = std::fs::read(&file.path) else {
            continue;
        };
        let Ok(tokens) = scan::tokenize(&src) else {
            continue;
        };
        let Some(entity) = super::sidecar::entity_element(&tokens, &src) else {
            continue;
        };
        let attr = |at: usize, name: &str| {
            scan::attribute(&src, &tokens[at], name)
                .ok()
                .flatten()
                .map(|value| scan::decode_entities(&String::from_utf8_lossy(value.of(&src))))
        };
        let terms = if attr(entity, "isDynamic").as_deref() == Some("true") {
            None
        } else {
            Some(
                scan::tags_within(&tokens, &src, "VocabularyTerm", entity)
                    .into_iter()
                    .filter_map(|term| attr(term, "name"))
                    .collect(),
            )
        };
        out.insert(file.info.name.clone(), terms);
    }
    out
}

/// An entity's `tags`: each `Vocabulary:Term`, and a term a fixed vocabulary of the solution
/// lacks. A vocabulary the solution does not define is the platform's or another solution's,
/// and is not judged.
fn check_tags(file: &str, src: &[u8], vocabularies: &Vocabularies) -> Vec<Problem> {
    // Nearly every entity has `tags=""`; only one with a value is worth reading.
    let tagged = src
        .windows(7)
        .position(|window| window[0].is_ascii_whitespace() && &window[1..] == b"tags=\"")
        .is_some_and(|at| src.get(at + 7) != Some(&b'"'));
    if !tagged {
        return Vec::new();
    }
    let Ok(tokens) = scan::tokenize(src) else {
        return Vec::new();
    };
    let Some(entity) = super::sidecar::entity_element(&tokens, src) else {
        return Vec::new();
    };
    let Some(tags) = scan::attribute(src, &tokens[entity], "tags")
        .ok()
        .flatten()
        .map(|value| scan::decode_entities(&String::from_utf8_lossy(value.of(src))))
    else {
        return Vec::new();
    };
    let mut problems = Vec::new();
    for tag in tags.split(';').map(str::trim).filter(|tag| !tag.is_empty()) {
        let Some((vocabulary, term)) = tag
            .split_once(':')
            .filter(|(vocabulary, term)| !vocabulary.is_empty() && !term.is_empty())
        else {
            problems.push(Problem {
                file: file.to_string(),
                rule: "malformed-tag",
                message: format!("tag {tag:?} is not Vocabulary:Term"),
            });
            continue;
        };
        if let Some(Some(terms)) = vocabularies.get(vocabulary) {
            if !terms.contains(term) {
                problems.push(Problem {
                    file: file.to_string(),
                    rule: "unknown-tag-term",
                    message: format!(
                        "model tag vocabulary {vocabulary} is not dynamic and has no term {term}; add the term to it or use one it has"
                    ),
                });
            }
        }
    }
    problems
}

/// A service declared without an implementation, or implemented without a declaration.
///
/// The second half is legitimate when the definition is inherited from a shape or template:
/// `inherited` says whether an ancestor is known to define it, and the solution can name the
/// others it knows are overridden that way: `Entity.Service` for one entity, or a bare `Service`
/// for that service wherever it is implemented.
fn check_services(
    file: &str,
    entity: &str,
    src: &[u8],
    allowed: &[String],
    inherited: &dyn Fn(&str) -> bool,
) -> Vec<Problem> {
    let Ok(extraction) = super::sidecar::extract(src) else {
        return Vec::new();
    };
    let mut problems = Vec::new();

    if !extraction.without_script.is_empty() {
        problems.push(Problem {
            file: file.to_string(),
            rule: "service-without-implementation",
            message: format!(
                "declared but never implemented: {}",
                extraction.without_script.join(", ")
            ),
        });
    }

    let listed = |service: &str| {
        allowed
            .iter()
            .any(|entry| entry == service || *entry == format!("{entity}.{service}"))
    };
    let unexplained: Vec<&String> = extraction
        .inherited
        .iter()
        .filter(|name| !listed(name) && !inherited(name))
        .collect();
    if !unexplained.is_empty() {
        let names: Vec<&str> = unexplained.iter().map(|n| n.as_str()).collect();
        problems.push(Problem {
            file: file.to_string(),
            rule: "service-without-definition",
            message: format!(
                "implemented here with no local definition: {}. If these override an inherited \
                 service, list them in [validate] inherited_overrides.",
                names.join(", ")
            ),
        });
    }
    problems
}

/// A mashup whose content payload will not parse.
///
/// The runtime reads that JSON. A mashup that will not parse shows an empty page rather than an
/// error, which is why this is worth catching before an import rather than after one.
fn check_mashup(file: &str, src: &[u8]) -> Vec<Problem> {
    let Some(payload) = mashup_payload(src) else {
        return Vec::new();
    };
    if payload.trim().is_empty() {
        return Vec::new();
    }

    match serde_json::from_str::<serde_json::Value>(payload.trim()) {
        Ok(value) => {
            // An object here, and its CSS a string when present: the runtime assumes both.
            match value.get("CustomMashupCss") {
                None | Some(serde_json::Value::Null) | Some(serde_json::Value::String(_)) => {
                    Vec::new()
                }
                Some(other) => vec![Problem {
                    file: file.to_string(),
                    rule: "mashup-css",
                    message: format!(
                        "CustomMashupCss must be a string when present, found {other}"
                    ),
                }],
            }
        }
        Err(e) => vec![Problem {
            file: file.to_string(),
            rule: "mashup-json",
            message: format!("mashupContent will not parse: {e}"),
        }],
    }
}

/// A Mashup entity's `mashupContent` payload, from CDATA or escaped text alike; `None` when the
/// file has no such element.
pub(crate) fn mashup_payload(src: &[u8]) -> Option<String> {
    let tokens = scan::tokenize(src).ok()?;
    let content = tokens
        .iter()
        .position(|t| t.kind == Kind::Start && t.name.of(src) == b"mashupContent")?;
    let end = scan::element_end_in(&tokens, src, content)?;
    Some(
        (content + 1..end)
            .filter_map(|i| match tokens[i].kind {
                Kind::Cdata => Some(String::from_utf8_lossy(tokens[i].inner.of(src)).into_owned()),
                Kind::Text => Some(scan::decode_entities(&String::from_utf8_lossy(
                    tokens[i].span.of(src),
                ))),
                _ => None,
            })
            .collect(),
    )
}

fn relative(solution: &Solution, entity: &EntityFile) -> String {
    entity
        .path
        .strip_prefix(&solution.root)
        .unwrap_or(&entity.path)
        .display()
        .to_string()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_inherited_override_is_allowed_by_its_qualified_or_bare_name() {
        let src = br#"<Entities><Things><Thing name="Acme.T"><ThingShape><ServiceDefinitions/><ServiceImplementations><ServiceImplementation name="Run" handlerName="Script"><ConfigurationTables><ConfigurationTable name="Script"><Rows><Row><code><![CDATA[1;]]></code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations></ThingShape></Thing></Things></Entities>"#;
        let rules = |allowed: &[&str]| -> Vec<&'static str> {
            let allowed: Vec<String> = allowed.iter().map(|a| a.to_string()).collect();
            check_services("t.xml", "Acme.T", src, &allowed, &|_| false)
                .iter()
                .map(|p| p.rule)
                .collect()
        };
        assert_eq!(rules(&[]), ["service-without-definition"]);
        assert!(
            rules(&["Acme.T.Run"]).is_empty(),
            "qualified, as documented"
        );
        assert!(
            rules(&["Run"]).is_empty(),
            "bare: the service on any entity"
        );
        assert_eq!(
            rules(&["Acme.Other.Run"]),
            ["service-without-definition"],
            "another entity's"
        );
    }

    #[test]
    fn member_names_cannot_clash_across_properties_services_and_events() {
        let src = br#"<Thing><ThingShape><PropertyDefinitions><PropertyDefinition name="Alpha"/></PropertyDefinitions><ServiceDefinitions><ServiceDefinition name="Alpha"/></ServiceDefinitions><EventDefinitions><EventDefinition name="Alpha"/></EventDefinitions></ThingShape></Thing>"#;
        let problems = check_member_names("T.xml", src);
        assert_eq!(problems.len(), 2);
        assert!(problems
            .iter()
            .all(|problem| problem.rule == "member-name-clash"));
    }

    /// An override needs no listing when an ancestor is known to define the service: a template
    /// of the solution, or one the platform cache holds. An unknown one is still reported.
    #[test]
    fn an_override_of_a_known_ancestor_service_needs_no_listing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("twaco.toml"), "[[project]]\nname = 'P'\n").unwrap();
        let implementation = |service: &str| {
            format!(
                r#"<ServiceImplementations><ServiceImplementation name="{service}" handlerName="Script"><ConfigurationTables><ConfigurationTable name="Script"><Rows><Row><code><![CDATA[1;]]></code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations>"#
            )
        };
        for (path, xml) in [
            (
                "ThingTemplates/P.Base_TT.xml",
                r#"<Entities><ThingTemplates><ThingTemplate name="P.Base_TT" projectName="P" baseThingTemplate="PTC.Base.EntryPoint_TT"><ServiceDefinitions><ServiceDefinition name="Local"><ResultType baseType="NOTHING" name="result"/></ServiceDefinition></ServiceDefinitions></ThingTemplate></ThingTemplates></Entities>"#.to_string(),
            ),
            (
                "ThingTemplates/P.EntryPoint_TT.xml",
                format!(
                    r#"<Entities><ThingTemplates><ThingTemplate name="P.EntryPoint_TT" projectName="P" baseThingTemplate="P.Base_TT">{}</ThingTemplate></ThingTemplates></Entities>"#,
                    [implementation("Local"), implementation("DeployComponent"), implementation("Unknown")]
                        .concat()
                        .replace("</ServiceImplementations><ServiceImplementations>", "")
                ),
            ),
        ] {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, xml).unwrap();
        }
        let solution = Solution::load(&dir.path().join("twaco.toml")).unwrap();
        let undefined = || -> Vec<String> {
            check(&solution)
                .into_iter()
                .filter(|p| p.rule == "service-without-definition")
                .map(|p| p.message)
                .collect()
        };
        assert_eq!(
            undefined(),
            ["implemented here with no local definition: DeployComponent, Unknown. If these override an inherited service, list them in [validate] inherited_overrides."]
        );
        std::fs::create_dir_all(dir.path().join(".twaco")).unwrap();
        std::fs::write(
            dir.path().join(".twaco/platform.json"),
            r#"{"templates":{"PTC.Base.EntryPoint_TT":{"services":{"DeployComponent":{"result":{"baseType":"BOOLEAN"}}}}}}"#,
        )
        .unwrap();
        assert_eq!(
            undefined(),
            ["implemented here with no local definition: Unknown. If these override an inherited service, list them in [validate] inherited_overrides."]
        );
    }

    #[test]
    fn tags_name_a_term_a_fixed_vocabulary_of_the_solution_has() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("twaco.toml"), "[[project]]\nname = 'P'\n").unwrap();
        for (path, xml) in [
            (
                "ModelTags/P.Fixed.xml",
                r#"<Entities><ModelTags><ModelTagVocabulary name="P.Fixed" projectName="P" isDynamic="false"><VocabularyTerm name="Known"/></ModelTagVocabulary></ModelTags></Entities>"#,
            ),
            (
                "ModelTags/P.Open.xml",
                r#"<Entities><ModelTags><ModelTagVocabulary name="P.Open" projectName="P" isDynamic="true"/></ModelTags></Entities>"#,
            ),
            (
                "Things/P.T.xml",
                r#"<Entities><Things><Thing name="P.T" projectName="P" tags="P.Fixed:Known;P.Fixed:Missing;P.Open:Anything;Applications:Mobile;loose"/></Things></Entities>"#,
            ),
        ] {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, xml).unwrap();
        }
        let solution = Solution::load(&dir.path().join("twaco.toml")).unwrap();
        let found: Vec<(&str, String)> = check(&solution)
            .into_iter()
            .filter(|problem| problem.rule.contains("tag"))
            .map(|problem| (problem.rule, problem.message))
            .collect();
        assert_eq!(
            found,
            [
                (
                    "unknown-tag-term",
                    "model tag vocabulary P.Fixed is not dynamic and has no term Missing; add the term to it or use one it has".to_string()
                ),
                ("malformed-tag", "tag \"loose\" is not Vocabulary:Term".to_string()),
            ]
        );
    }

    #[test]
    fn a_mashup_with_valid_json_is_accepted() {
        let src = br#"<Entities><Mashups><Mashup name="M"><mashupContent><![CDATA[{"UI":{}}]]></mashupContent></Mashup></Mashups></Entities>"#;
        assert!(check_mashup("m.xml", src).is_empty());
    }

    #[test]
    fn a_mashup_whose_json_will_not_parse_is_reported() {
        let src = br#"<Entities><Mashups><Mashup name="M"><mashupContent><![CDATA[{"UI":}]]></mashupContent></Mashup></Mashups></Entities>"#;
        let problems = check_mashup("m.xml", src);
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].rule, "mashup-json");
    }

    #[test]
    fn a_non_string_css_is_reported() {
        let src = br#"<Entities><Mashups><Mashup name="M"><mashupContent><![CDATA[{"CustomMashupCss": 3}]]></mashupContent></Mashup></Mashups></Entities>"#;
        assert_eq!(check_mashup("m.xml", src)[0].rule, "mashup-css");
    }

    #[test]
    fn an_absent_css_is_fine() {
        let src = br#"<Entities><Mashups><Mashup name="M"><mashupContent><![CDATA[{"CustomMashupCss": null}]]></mashupContent></Mashup></Mashups></Entities>"#;
        assert!(check_mashup("m.xml", src).is_empty());
    }

    #[test]
    fn an_empty_payload_is_not_a_parse_failure() {
        let src = br#"<Entities><Mashups><Mashup name="M"><mashupContent></mashupContent></Mashup></Mashups></Entities>"#;
        assert!(check_mashup("m.xml", src).is_empty());
    }
}
