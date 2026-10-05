//! Whether the entity files in a solution are internally consistent.
//!
//! These are the checks that catch a repository which looks fine and imports wrong: a file whose
//! name disagrees with the entity inside it, two files claiming the same entity, a service half
//! declared, a mashup whose JSON payload will not parse.

use super::config::Solution;
use super::scan::{self, Kind};
use super::workspace::{self, EntityFile};
use std::collections::BTreeMap;

/// One inconsistency, with the file it is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub file: String,
    pub rule: &'static str,
    pub message: String,
}

/// Check every entity in the solution.
pub fn check(solution: &Solution) -> Vec<Problem> {
    let found = workspace::discover(solution);
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

    for entity in &found.entities {
        problems.extend(check_one(solution, entity));
    }

    for problem in &found.unreadable {
        problems.push(Problem {
            file: problem.split(':').next().unwrap_or(problem).to_string(),
            rule: "unreadable",
            message: problem.clone(),
        });
    }
    problems
}

fn check_one(solution: &Solution, entity: &EntityFile) -> Vec<Problem> {
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
    ));
    if entity.info.collection == "Mashups" {
        problems.extend(check_mashup(&file, &src));
    }
    problems
}

/// A service declared without an implementation, or implemented without a declaration.
///
/// The second half is legitimate when the definition is inherited from a shape or template, so
/// the solution can name the services it knows are overridden that way: `Entity.Service` for
/// one entity, or a bare `Service` for that service wherever it is implemented.
fn check_services(file: &str, entity: &str, src: &[u8], allowed: &[String]) -> Vec<Problem> {
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
        .filter(|name| !listed(name))
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
    let Ok(tokens) = scan::tokenize(src) else {
        return Vec::new();
    };
    let Some(content) = tokens
        .iter()
        .position(|t| t.kind == Kind::Start && t.name.of(src) == b"mashupContent")
    else {
        return Vec::new();
    };
    let Some(end) = scan::element_end_in(&tokens, src, content) else {
        return Vec::new();
    };

    let payload: String = (content + 1..end)
        .filter_map(|i| match tokens[i].kind {
            Kind::Cdata => Some(String::from_utf8_lossy(tokens[i].inner.of(src)).into_owned()),
            Kind::Text => Some(scan::decode_entities(&String::from_utf8_lossy(
                tokens[i].span.of(src),
            ))),
            _ => None,
        })
        .collect();
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
            check_services("t.xml", "Acme.T", src, &allowed)
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
