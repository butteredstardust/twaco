//! A THINGNAME property default that names a Thing of the delete set, on an entity that Thing
//! depends on, is a cycle the server cannot delete in any order: each side is "Referenced By"
//! the other (verified on a 10.2 server, 2026-10-30). Importing the entity again without those
//! defaults breaks it.

use super::remote::Remote;
use super::report::{EntityResult, Status};
use crate::core::entity::key::EntityKey;
use crate::core::scan::{self, Kind};
use crate::core::server::ServerError;
use crate::core::splice::{self, Edit};
use std::collections::BTreeSet;

/// For each Ready entity in a cycle through a THINGNAME default, its export without the
/// defaults that close the cycle; the Thing's dependence on it is dropped, so the order puts the
/// Thing first.
pub(super) fn plan(
    remote: &dyn Remote,
    entities: &mut [EntityResult],
) -> Result<(), (String, ServerError)> {
    let ready = |entity: &EntityResult| entity.status == Status::Ready;
    let set: BTreeSet<EntityKey> = entities
        .iter()
        .filter(|entity| ready(entity))
        .map(EntityResult::key)
        .collect();
    for index in 0..entities.len() {
        let entity = &entities[index];
        if !ready(entity)
            || !matches!(
                entity.collection.as_str(),
                "Things" | "ThingTemplates" | "ThingShapes"
            )
        {
            continue;
        }
        let key = entity.key();
        // The Things of the set that this entity depends on and that depend on it in turn.
        let things: BTreeSet<String> = entities
            .iter()
            .filter(|thing| ready(thing) && thing.collection == "Things" && thing.key() != key)
            .filter(|thing| depends_on(thing, &key))
            .filter(|thing| reaches(entities, &set, &key, &thing.key()))
            .map(|thing| thing.name.clone())
            .collect();
        if things.is_empty() {
            continue;
        }
        let Some(export) = remote.export(&key).map_err(|why| (key.to_string(), why))? else {
            continue;
        };
        let Some((stripped, cleared)) = without_defaults(&export, &things) else {
            continue;
        };
        for (_, named) in &cleared {
            if let Some(thing) = entities
                .iter_mut()
                .find(|other| other.collection == "Things" && other.name == *named)
            {
                thing.dependents.retain(|dependent| {
                    (dependent.collection.as_str(), dependent.name.as_str())
                        != (key.collection(), key.name())
                });
            }
        }
        let entity = &mut entities[index];
        entity.warnings.extend(cleared.iter().map(|(property, thing)| {
            format!("clears the default of property {property} (Things/{thing}, which depends on this entity) before deleting, to break the cycle")
        }));
        entity.unlink = Some(stripped);
    }
    Ok(())
}

/// Whether `thing` lists `key` among its dependents: `key` depends on `thing`.
fn depends_on(thing: &EntityResult, key: &EntityKey) -> bool {
    thing
        .dependents
        .iter()
        .any(|dependent| dependent.collection == key.collection() && dependent.name == key.name())
}

/// Whether `to` depends on `from` through the set: `to` is reached from `from`'s dependents.
fn reaches(
    entities: &[EntityResult],
    set: &BTreeSet<EntityKey>,
    from: &EntityKey,
    to: &EntityKey,
) -> bool {
    let mut seen = BTreeSet::new();
    let mut next = vec![from.clone()];
    while let Some(key) = next.pop() {
        let Some(entity) = entities.iter().find(|entity| entity.key() == key) else {
            continue;
        };
        for dependent in &entity.dependents {
            let Ok(dependent) = EntityKey::new(&dependent.collection, &dependent.name) else {
                continue;
            };
            if &dependent == to {
                return true;
            }
            if set.contains(&dependent) && seen.insert(dependent.clone()) {
                next.push(dependent);
            }
        }
    }
    false
}

/// A property whose default was emptied, and the Thing it named.
type Cleared = (String, String);

/// `xml` with every THINGNAME `aspect.defaultValue` that names one of `things` emptied, and the
/// (property, Thing) pairs it cleared; `None` when there is none or the XML does not scan.
fn without_defaults(xml: &[u8], things: &BTreeSet<String>) -> Option<(Vec<u8>, Vec<Cleared>)> {
    let tokens = scan::tokenize(xml).ok()?;
    let mut edits = Vec::new();
    let mut cleared = Vec::new();
    for token in &tokens {
        if !matches!(token.kind, Kind::Start | Kind::Empty)
            || token.name.of(xml) != b"PropertyDefinition"
        {
            continue;
        }
        let attributes = scan::attributes(xml, token).ok()?;
        let value = |wanted: &[u8]| {
            attributes
                .iter()
                .find(|attribute| attribute.name.of(xml) == wanted)
        };
        if value(b"baseType").map(|attribute| attribute.value.of(xml)) != Some(b"THINGNAME") {
            continue;
        }
        let Some(default) = value(b"aspect.defaultValue") else {
            continue;
        };
        let named = String::from_utf8_lossy(default.value.of(xml)).into_owned();
        if !things.contains(&named) {
            continue;
        }
        let property = value(b"name")
            .map(|attribute| String::from_utf8_lossy(attribute.value.of(xml)).into_owned())
            .unwrap_or_default();
        edits.push(Edit::new(default.value, Vec::new()));
        cleared.push((property, named));
    }
    if edits.is_empty() {
        return None;
    }
    Some((splice::splice(xml, &edits).ok()?, cleared))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_thingname_defaults_naming_a_thing_of_the_set_are_emptied() {
        let xml = br#"<Entities><ThingTemplates><ThingTemplate name="T_TT"><PropertyDefinitions><PropertyDefinition aspect.defaultValue="A" baseType="THINGNAME" name="me"/><PropertyDefinition aspect.defaultValue="B" baseType="THINGNAME" name="other"/><PropertyDefinition aspect.defaultValue="A" baseType="STRING" name="label"></PropertyDefinition></PropertyDefinitions></ThingTemplate></ThingTemplates></Entities>"#;
        let (stripped, cleared) =
            without_defaults(xml, &BTreeSet::from(["A".to_string()])).unwrap();
        let stripped = String::from_utf8(stripped).unwrap();
        assert!(stripped.contains(r#"aspect.defaultValue="" baseType="THINGNAME" name="me""#));
        assert!(stripped.contains(r#"aspect.defaultValue="B" baseType="THINGNAME""#));
        assert!(stripped.contains(r#"aspect.defaultValue="A" baseType="STRING""#));
        assert_eq!(cleared, [("me".to_string(), "A".to_string())]);
        assert!(without_defaults(xml, &BTreeSet::from(["C".to_string()])).is_none());
    }
}
