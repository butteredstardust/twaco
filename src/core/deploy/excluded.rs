//! What still references an entity a deploy leaves out.
//!
//! The Importer is all or nothing, so an entity left out while another entity of the import
//! points at it fails the whole import, one refusal per round. This finds those entities before
//! anything is sent, following only declared structural reference fields.

use super::super::scan::{self, Kind};
use super::super::workspace::EntityFile;
use std::collections::{BTreeMap, BTreeSet};

/// Structural references retain their collection; prose and service bodies are never edges.
fn mentions(bytes: &[u8]) -> BTreeSet<String> {
    let Ok(tokens) = scan::tokenize(bytes) else {
        return BTreeSet::new();
    };
    let mut found = BTreeSet::new();
    let mut parents: Vec<usize> = Vec::new();
    for (at, token) in tokens.iter().enumerate() {
        match token.kind {
            Kind::Start | Kind::Empty => {
                let tag = token.name.of(bytes);
                let attr = |name| {
                    scan::attribute(bytes, token, name)
                        .ok()
                        .flatten()
                        .map(|span| scan::decode_entities(&String::from_utf8_lossy(span.of(bytes))))
                        .unwrap_or_default()
                };
                for attribute in scan::attributes(bytes, token)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|_| tag != b"Owner")
                {
                    if let Some(collection) =
                        super::super::requirements::reference_attribute(attribute.name.of(bytes))
                    {
                        let value = scan::decode_entities(&String::from_utf8_lossy(
                            attribute.value.of(bytes),
                        ));
                        if !value.is_empty() {
                            found.insert(format!("{collection}/{value}"));
                        }
                    }
                }
                let collection = match tag {
                    b"ImplementedShape" => Some("ThingShapes"),
                    b"ConfigurationTable" => {
                        let value = attr("dataShapeName");
                        if !value.is_empty() {
                            found.insert(format!("DataShapes/{value}"));
                        }
                        None
                    }
                    _ => None,
                };
                if let Some(collection) = collection {
                    let name = attr("name");
                    if !name.is_empty() {
                        found.insert(format!("{collection}/{name}"));
                    }
                }
                let in_configuration = parents
                    .iter()
                    .any(|&at| tokens[at].name.of(bytes) == b"ConfigurationTables");
                let in_service = parents.iter().any(|&at| {
                    matches!(
                        tokens[at].name.of(bytes),
                        b"ServiceImplementation" | b"Service"
                    )
                });
                if let Some(&parent) = parents.last() {
                    let parent_tag = tokens[parent].name.of(bytes);
                    if !in_service
                        && (parent_tag == b"ThingProperties"
                            || (in_configuration && parent_tag == b"Row"))
                    {
                        let collection = match tag {
                            b"dataShape" => Some("DataShapes"),
                            b"queueProvider" | b"QueueProvider" => Some("QueueProviders"),
                            b"Stream" => Some("Streams"),
                            b"iotStream" | b"IoTStream" | b"valueStream" => Some("Things"),
                            _ => None,
                        };
                        if let Some(collection) = collection {
                            if let Some(value) = super::super::agents::child_text(
                                &tokens,
                                bytes,
                                std::str::from_utf8(tag).unwrap(),
                                parent,
                            )
                            .filter(|value| {
                                !value.is_empty() && !value.contains(char::is_whitespace)
                            }) {
                                found.insert(format!("{collection}/{value}"));
                            }
                        }
                    }
                }
                if token.kind == Kind::Start {
                    parents.push(at);
                }
            }
            Kind::End => {
                parents.pop();
            }
            _ => {}
        }
    }
    found
}

/// For each excluded entity, the `Collection/Name` of every remaining entity that references it,
/// directly or through another referrer, in name order. Excluded entities with no referrers are
/// left out of the answer.
pub(crate) fn referrers(
    remaining: &[EntityFile],
    excluded: &[EntityFile],
) -> BTreeMap<String, Vec<String>> {
    let mentioned: Vec<(String, BTreeSet<String>)> = remaining
        .iter()
        .map(|entity| {
            let names = std::fs::read(&entity.path)
                .map(|bytes| mentions(&bytes))
                .unwrap_or_default();
            (
                format!("{}/{}", entity.info.collection, entity.info.name),
                names,
            )
        })
        .collect();
    let mut answer = BTreeMap::new();
    for root in excluded {
        let mut reached: BTreeSet<String> = BTreeSet::new();
        let mut frontier = vec![format!("{}/{}", root.info.collection, root.info.name)];
        let mut seen: BTreeSet<String> = frontier.iter().cloned().collect();
        while let Some(name) = frontier.pop() {
            for (label, names) in &mentioned {
                if names.contains(&name) && reached.insert(label.clone()) {
                    let own = label.clone();
                    if seen.insert(own.clone()) {
                        frontier.push(own);
                    }
                }
            }
        }
        if !reached.is_empty() {
            answer.insert(
                format!("{}/{}", root.info.collection, root.info.name),
                reached.into_iter().collect(),
            );
        }
    }
    answer
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(dir: &std::path::Path, collection: &str, name: &str, body: &str) -> EntityFile {
        let path = dir.join(format!("{collection}-{name}.xml"));
        std::fs::write(&path, body).unwrap();
        EntityFile {
            path,
            found_under: "Acme".into(),
            info: super::super::super::entity::EntityInfo {
                collection: collection.into(),
                name: name.into(),
                project: "Acme".into(),
            },
        }
    }

    #[test]
    fn a_stream_and_its_thing_are_found_behind_an_excluded_queue_provider() {
        let dir = tempfile::tempdir().unwrap();
        let queue = file(
            dir.path(),
            "QueueProviders",
            "Acme.Queue",
            r#"<Entities><QueueProviders><QueueProvider name="Acme.Queue" projectName="Acme"/></QueueProviders></Entities>"#,
        );
        let stream = file(
            dir.path(),
            "Streams",
            "Acme.Stream",
            r#"<Entities><Streams><Stream name="Acme.Stream" projectName="Acme" queueProvider="Acme.Queue"/></Streams></Entities>"#,
        );
        let thing = file(
            dir.path(),
            "Things",
            "Acme.Pump",
            r#"<Entities><Things><Thing name="Acme.Pump" projectName="Acme"><ThingProperties><Stream>Acme.Stream</Stream></ThingProperties></Thing></Things></Entities>"#,
        );
        let unrelated = file(
            dir.path(),
            "Things",
            "Acme.Other",
            r#"<Entities><Things><Thing name="Acme.Other" projectName="Acme"><Service><![CDATA[Things["Acme.Queue"]]]></Service></Thing></Things></Entities>"#,
        );
        let found = referrers(&[stream, thing, unrelated], &[queue]);
        assert_eq!(
            found["QueueProviders/Acme.Queue"],
            ["Streams/Acme.Stream", "Things/Acme.Pump"]
        );
    }

    #[test]
    fn a_configuration_table_value_in_cdata_is_a_reference() {
        let dir = tempfile::tempdir().unwrap();
        let shape = file(dir.path(), "DataShapes", "Acme.Shape", "<Entities/>");
        let stream = file(
            dir.path(),
            "Things",
            "Acme.Stream",
            r#"<Entities><Things><Thing name="Acme.Stream" projectName="Acme"><ConfigurationTables><ConfigurationTable name="StreamProcessingSettings"><Rows><Row><dataShape><![CDATA[Acme.Shape]]></dataShape></Row></Rows></ConfigurationTable></ConfigurationTables></Thing></Things></Entities>"#,
        );
        let found = referrers(&[stream], &[shape]);
        assert_eq!(found["DataShapes/Acme.Shape"], ["Things/Acme.Stream"]);
    }

    #[test]
    fn prose_and_arbitrary_text_are_not_references() {
        let dir = tempfile::tempdir().unwrap();
        let queue = file(dir.path(), "QueueProviders", "Acme.Queue", "<Entities/>");
        let other = file(
            dir.path(),
            "Things",
            "Acme.Other",
            r#"<Entities><Things><Thing name="Acme.Other" description="Acme.Queue" documentation="Acme.Queue" friendlyName="Acme.Queue" aspect.label="Acme.Queue" tags="Acme.Queue"><Documentation>Acme.Queue</Documentation><Service><![CDATA[Acme.Queue]]></Service><ConfigurationTables><ConfigurationTable name="Settings"><Rows><Row><description><![CDATA[Acme.Queue]]></description></Row></Rows></ConfigurationTable></ConfigurationTables></Thing></Things></Entities>"#,
        );
        assert!(referrers(&[other], &[queue]).is_empty());
    }

    #[test]
    fn typed_references_do_not_conflate_collections_or_ignore_equal_names() {
        let dir = tempfile::tempdir().unwrap();
        let queue = file(dir.path(), "QueueProviders", "Acme.Shared", "<Entities/>");
        let shape = file(dir.path(), "DataShapes", "Acme.Shared", "<Entities/>");
        let thing = file(
            dir.path(),
            "Things",
            "Acme.Shared",
            r#"<Entities><Things><Thing name="Acme.Shared" queueProvider="Acme.Shared"/></Things></Entities>"#,
        );
        let found = referrers(&[thing], &[queue, shape]);
        assert_eq!(found.len(), 1);
        assert_eq!(found["QueueProviders/Acme.Shared"], ["Things/Acme.Shared"]);
    }

    #[test]
    fn configuration_cells_follow_the_queue_to_iot_stream_to_thing_chain() {
        let dir = tempfile::tempdir().unwrap();
        let queue = file(dir.path(), "QueueProviders", "Acme.Queue", "<Entities/>");
        let stream = file(
            dir.path(),
            "Things",
            "Acme.Stream",
            r#"<Entities><Things><Thing name="Acme.Stream"><ConfigurationTables><ConfigurationTable name="StreamProcessingSettings"><Rows><Row><queueProvider><![CDATA[Acme.Queue]]></queueProvider></Row></Rows></ConfigurationTable></ConfigurationTables></Thing></Things></Entities>"#,
        );
        let thing = file(
            dir.path(),
            "Things",
            "Acme.Pump",
            r#"<Entities><Things><Thing name="Acme.Pump"><ConfigurationTables><ConfigurationTable name="Settings"><Rows><Row><iotStream><![CDATA[Acme.Stream]]></iotStream></Row></Rows></ConfigurationTable></ConfigurationTables></Thing></Things></Entities>"#,
        );
        assert_eq!(
            referrers(&[stream, thing], &[queue])["QueueProviders/Acme.Queue"],
            ["Things/Acme.Pump", "Things/Acme.Stream"]
        );
    }

    #[test]
    fn nothing_is_reported_when_nothing_references_the_excluded_entity() {
        let dir = tempfile::tempdir().unwrap();
        let queue = file(dir.path(), "QueueProviders", "Acme.Queue", "<Entities/>");
        let other = file(
            dir.path(),
            "Things",
            "Acme.Other",
            r#"<Entities><Things><Thing name="Acme.Other" projectName="Acme.Queue"/></Things></Entities>"#,
        );
        assert!(referrers(&[other], &[queue]).is_empty());
    }
}
