use super::super::config::Solution;
use super::super::scan::{self, Kind};
use super::super::workspace;
use super::model::DbError;
use std::collections::BTreeMap;

/// Resolve the database Thing from local inheritance unless the caller named one explicitly.
pub fn resolve_thing(solution: &Solution, explicit: Option<&str>) -> Result<String, DbError> {
    if let Some(name) = explicit {
        if name.is_empty() {
            return Err(DbError("--thing needs a non-empty Thing name".to_string()));
        }
        return Ok(name.to_string());
    }
    let discovery = workspace::discover(solution);
    if !discovery.unreadable.is_empty() {
        return Err(DbError(format!(
            "cannot choose a database Thing while entity files are unreadable: {}",
            discovery.unreadable.join("; ")
        )));
    }
    let mut bases = BTreeMap::new();
    let mut things = Vec::new();
    for entity in discovery.entities {
        let bytes = std::fs::read(&entity.path)
            .map_err(|error| DbError(format!("{}: {error}", entity.path.display())))?;
        let tokens = scan::tokenize(&bytes)
            .map_err(|error| DbError(format!("{}: {error}", entity.path.display())))?;
        let tag = tokens
            .iter()
            .filter(|token| matches!(token.kind, Kind::Start | Kind::Empty))
            .nth(2)
            .ok_or_else(|| DbError(format!("{}: entity tag is missing", entity.path.display())))?;
        if entity.info.collection == "ThingTemplates" {
            let base = attribute(&bytes, tag, "baseThingTemplate")?;
            bases.insert(entity.info.name, base);
        } else if entity.info.collection == "Things" {
            things.push((entity.info.name, attribute(&bytes, tag, "thingTemplate")?));
        }
    }
    let inherits_database = |template: &str| {
        let mut current = template;
        let mut seen = std::collections::BTreeSet::new();
        loop {
            if current == "Database" || current.ends_with("Database_TT") {
                return true;
            }
            if !seen.insert(current.to_string()) {
                return false;
            }
            let Some(parent) = bases.get(current) else {
                return false;
            };
            current = parent;
        }
    };
    let candidates: Vec<String> = things
        .into_iter()
        .filter(|(_, template)| inherits_database(template))
        .map(|(name, _)| name)
        .collect();
    match candidates.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(DbError(
            "no database Thing was found in the solution (candidates: none); pass --thing <name>"
                .to_string(),
        )),
        _ => Err(DbError(format!(
            "several database Things were found: {}; pass --thing <name>",
            candidates.join(", ")
        ))),
    }
}

fn attribute(bytes: &[u8], tag: &scan::Token, name: &str) -> Result<String, DbError> {
    scan::attribute(bytes, tag, name)
        .map_err(|error| DbError(error.to_string()))
        .map(|span| {
            span.map(|span| scan::decode_entities(&String::from_utf8_lossy(span.of(bytes))))
                .unwrap_or_default()
        })
}
