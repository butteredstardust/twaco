//! Live model questions the repository cannot answer: how many Things a template or shape has on
//! the server (`model resolve`), and what an entity is made of there, member by member, with
//! where each member comes from and, if asked, its current value (`entity inspect`). Read-only.

use super::entity::key::ServiceTarget;
use super::types::platform::Remote;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// The most instances listed unless more are asked for; the total is always given.
pub const DEFAULT_LIMIT: usize = 50;

#[derive(Debug, Serialize)]
pub struct Instances {
    pub entity: String,
    /// Every Thing that implements it, on the server.
    pub total: u64,
    /// The first of them, at most the limit asked for.
    pub things: Vec<String>,
    pub truncated: bool,
}

/// The Things that implement a ThingTemplate or ThingShape on the server.
pub fn resolve(
    remote: &dyn Remote,
    collection: &str,
    name: &str,
    limit: usize,
) -> Result<Instances, String> {
    if !matches!(collection, "ThingTemplates" | "ThingShapes") {
        return Err(format!(
            "{collection}/{name}: model resolve counts the Things of a ThingTemplate or a ThingShape"
        ));
    }
    let entity = format!("{collection}/{name}");
    let target = ServiceTarget::entity(collection, name).map_err(|e| e.to_string())?;
    let reply = remote
        .call(
            &target,
            "QueryImplementingThingsOptimizedWithTotalCount",
            &json!({ "maxItems": limit }),
        )
        .map_err(|error| {
            format!("{entity}.QueryImplementingThingsOptimizedWithTotalCount: {error}")
        })?
        .ok_or_else(|| format!("{entity}: the server gave no answer"))?;
    let row = &reply["rows"][0];
    let total = row["totalCount"]
        .as_f64()
        .map(super::number::saturating_u64)
        .ok_or_else(|| format!("{entity}: the answer has no totalCount: {reply}"))?;
    let mut things: Vec<String> = row["rootEntityList"]["rows"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|thing| thing["name"].as_str().map(str::to_string))
        .collect();
    things.sort();
    things.truncate(limit);
    Ok(Instances {
        entity,
        truncated: (things.len() as u64) < total,
        total,
        things,
    })
}

#[derive(Debug, Serialize)]
pub struct Member {
    pub kind: &'static str,
    pub name: String,
    /// What declares it: the entity itself, a template, a shape or a platform package.
    pub source: String,
    pub source_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_type: Option<String>,
    /// A property's current value, when values were asked for and the caller may read it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct Inspection {
    pub entity: String,
    pub members: Vec<Member>,
    /// How many members each source gives, by `source_type source`.
    pub sources: BTreeMap<String, usize>,
    /// Properties whose value was asked for but the server did not return (no read permission).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unreadable: Vec<String>,
}

/// The members of an entity as the server has it, each with its source, and property values if
/// `values`. `member` narrows to the members of that name.
pub fn inspect(
    remote: &dyn Remote,
    collection: &str,
    name: &str,
    member: Option<&str>,
    values: bool,
) -> Result<Inspection, String> {
    let entity = format!("{collection}/{name}");
    let target = ServiceTarget::entity(collection, name).map_err(|e| e.to_string())?;
    let metadata = remote
        .call(&target, "GetMetadataAsJSON", &json!({}))
        .map_err(|error| format!("{entity}.GetMetadataAsJSON: {error}"))?
        .ok_or_else(|| format!("{entity}: the server gave no metadata"))?;
    let current = if values && collection == "Things" {
        remote
            .call(&target, "GetPropertyValues", &json!({}))
            .map_err(|error| format!("{entity}.GetPropertyValues: {error}"))?
            .map(|reply| reply["rows"][0].clone())
    } else {
        None
    };
    let mut members = Vec::new();
    let mut unreadable = Vec::new();
    for (kind, key) in [
        ("property", "propertyDefinitions"),
        ("service", "serviceDefinitions"),
        ("event", "eventDefinitions"),
    ] {
        for (member_name, definition) in metadata[key].as_object().into_iter().flatten() {
            if member.is_some_and(|wanted| wanted != member_name) {
                continue;
            }
            let value = match (&current, kind) {
                (Some(row), "property") => match row.get(member_name) {
                    Some(value) => Some(value.clone()),
                    None => {
                        unreadable.push(member_name.clone());
                        None
                    }
                },
                _ => None,
            };
            members.push(Member {
                kind,
                name: member_name.clone(),
                source: definition["sourceName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                source_type: definition["sourceType"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                base_type: definition["baseType"].as_str().map(str::to_string),
                value,
            });
        }
    }
    if let (Some(wanted), true) = (member, members.is_empty()) {
        return Err(format!(
            "{entity} has no property, service or event {wanted} on the server"
        ));
    }
    members.sort_by(|a, b| (a.kind, &a.name).cmp(&(b.kind, &b.name)));
    let mut sources = BTreeMap::new();
    for found in &members {
        *sources
            .entry(format!("{} {}", found.source_type, found.source))
            .or_insert(0) += 1;
    }
    Ok(Inspection {
        entity,
        members,
        sources,
        unreadable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::server::ServerError;

    struct Fake(BTreeMap<String, Value>);

    impl Remote for Fake {
        fn call(
            &self,
            target: &ServiceTarget,
            service: &str,
            _: &Value,
        ) -> Result<Option<Value>, ServerError> {
            Ok(self.0.get(&format!("{target}.{service}")).cloned())
        }
    }

    #[test]
    fn resolve_gives_the_total_and_the_first_things() {
        let fake = Fake(BTreeMap::from([(
            "ThingTemplates/Acme.TT.QueryImplementingThingsOptimizedWithTotalCount".to_string(),
            json!({ "rows": [{ "totalCount": 3.0, "rootEntityList": { "rows": [{ "name": "B" }, { "name": "A" }] } }] }),
        )]));
        let found = resolve(&fake, "ThingTemplates", "Acme.TT", 2).unwrap();
        assert_eq!(
            (found.total, found.things.clone(), found.truncated),
            (3, vec!["A".into(), "B".into()], true)
        );
        assert!(resolve(&fake, "Things", "X", 2).is_err());
    }

    #[test]
    fn inspect_names_each_members_source_and_unreadable_values() {
        let fake = Fake(BTreeMap::from([
            (
                "Things/Acme.T.GetMetadataAsJSON".to_string(),
                json!({
                    "propertyDefinitions": {
                        "Title": { "sourceName": "Acme.T", "sourceType": "Thing", "baseType": "STRING" },
                        "Secret": { "sourceName": "Acme.TT", "sourceType": "ThingTemplate", "baseType": "STRING" }
                    },
                    "serviceDefinitions": { "Run": { "sourceName": "Acme.TS", "sourceType": "ThingShape" } },
                    "eventDefinitions": {}
                }),
            ),
            (
                "Things/Acme.T.GetPropertyValues".to_string(),
                json!({ "rows": [{ "Title": "Orders" }] }),
            ),
        ]));
        let found = inspect(&fake, "Things", "Acme.T", None, true).unwrap();
        let names: Vec<_> = found
            .members
            .iter()
            .map(|m| (m.kind, m.name.as_str(), m.source.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                ("property", "Secret", "Acme.TT"),
                ("property", "Title", "Acme.T"),
                ("service", "Run", "Acme.TS")
            ]
        );
        assert_eq!(found.members[1].value, Some(json!("Orders")));
        assert_eq!(found.unreadable, ["Secret"]);
        assert_eq!(found.sources["ThingShape Acme.TS"], 1);
        assert!(inspect(&fake, "Things", "Acme.T", Some("Nope"), false).is_err());
    }
}
