//! The services every Thing, ThingTemplate and ThingShape has, from the platform's own classes.
//!
//! `builtin_services.json` was read from the `@ThingworxServiceDefinition` methods of ThingWorx
//! 10.2.0-b75 (`RootEntity`, `Thing`, `ThingTemplate`, `ThingShape` in platform-common) with
//! their parameters and result types. It fills what the optional platform cache cannot: a
//! ThingTemplate's own `GetImplementingThingsWithData` is not in any Thing's metadata, and the
//! cache is absent until `types --platform` has run. A solution's own service, and the cache,
//! always win over this table.
//!
//! `GetProperties` is the mashup runtime's pseudo-service (it calls `GetNamedProperties`), not a
//! platform method. Services whose rows are the template's properties
//! (`GetImplementingThingsWithData`, `QueryImplementingThingsWithData`, ...) and the ones that
//! return a Thing's property values carry no DataShape: their fields vary with the entity.

use super::types::{Parameter, Service, TypedValue};
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::OnceLock;

static TABLE: OnceLock<Value> = OnceLock::new();

fn table() -> &'static Value {
    TABLE.get_or_init(|| {
        serde_json::from_str(include_str!("builtin_services.json"))
            .expect("builtin_services.json is valid JSON")
    })
}

/// The groups to search for an entity of `collection`, nearest first.
fn groups(collection: &str) -> &'static [&'static str] {
    match collection {
        "Things" => &["thing", "root"],
        // A dynamic source on a template or shape calls the Things' services too.
        "ThingTemplates" => &["template", "thing", "root"],
        "ThingShapes" => &["shape", "thing", "root"],
        "Networks" => &["network", "root"],
        _ => &[],
    }
}

/// The platform service `name` on an entity of `collection` (`Things`, `ThingTemplates` or
/// `ThingShapes`), or `None` when the platform has no such service for it.
pub(crate) fn find(collection: &str, name: &str) -> Option<Service> {
    find_in(groups(collection), name)
}

/// The services a platform template adds to its Things (`Wiki`), or `None` for any other.
pub(crate) fn find_for_template(template: &str, name: &str) -> Option<Service> {
    match template {
        "Wiki" => find_in(&["wiki"], name),
        _ => None,
    }
}

fn find_in(groups: &[&str], name: &str) -> Option<Service> {
    let table = table();
    let raw = groups
        .iter()
        .find_map(|group| table.get(*group).and_then(|group| group.get(name)))?;
    let text = |value: &Value| value.as_str().map(str::to_string);
    let parameters = raw[0]
        .as_array()?
        .iter()
        .map(|input| Parameter {
            name: text(&input[0]).unwrap_or_default(),
            description: String::new(),
            value: TypedValue {
                base_type: text(&input[1]).unwrap_or_default(),
                data_shape: text(&input[2]),
            },
            required: input[3].as_bool().unwrap_or(false),
            default: None,
        })
        .collect();
    let result = &raw[1];
    Some(Service {
        name: name.to_string(),
        description: String::new(),
        parameters,
        result: TypedValue {
            base_type: text(&result[0]).unwrap_or_else(|| "NOTHING".into()),
            data_shape: text(&result[1]),
        },
    })
}

/// The DataShape names the platform's own services return or take, which twaco has no field
/// list for: a binding to one of their fields is unchecked rather than refused.
pub(crate) fn is_platform_shape(name: &str) -> bool {
    static SHAPES: OnceLock<BTreeSet<String>> = OnceLock::new();
    SHAPES
        .get_or_init(|| {
            let mut shapes = BTreeSet::new();
            let table = table();
            for group in ["root", "thing", "template", "shape"] {
                for service in table[group]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .map(|(_, v)| v)
                {
                    for input in service[0].as_array().into_iter().flatten() {
                        shapes.extend(input[2].as_str().map(str::to_string));
                    }
                    shapes.extend(service[1][1].as_str().map(str::to_string));
                }
            }
            shapes
        })
        .contains(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_runtime_services_take_what_the_runtime_reads() {
        let get = find("Things", "GetProperties").unwrap();
        let names: Vec<_> = get.parameters.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["useWebsockets", "showSubscriptionMsg"]);
        assert!(find("Things", "SetProperties").is_some());
        assert!(find("Networks", "GetNetworkConnections").is_some());
        assert!(find_for_template("Wiki", "GetWikiPages").is_some());
        assert!(find_for_template("Thing", "GetWikiPages").is_none());
    }

    #[test]
    fn every_entity_kind_resolves_its_own_and_shared_services() {
        let rows = find("ThingTemplates", "GetImplementingThingsWithData").unwrap();
        assert_eq!(rows.result.base_type, "INFOTABLE");
        assert_eq!(rows.result.data_shape, None, "rows vary with the template");
        assert!(rows.parameters.is_empty());
        let query = find("ThingTemplates", "QueryImplementingThingsWithData").unwrap();
        assert!(query.parameters.iter().any(|p| p.name == "nameMask"));
        assert!(find("ThingShapes", "GetImplementingThings").is_some());
        assert!(find("Things", "GetImplementingThings").is_none());
        for collection in ["Things", "ThingTemplates", "ThingShapes"] {
            assert!(find(collection, "GetProperties").is_some(), "{collection}");
            assert!(find(collection, "GetMetadata").is_some(), "{collection}");
            assert!(find(collection, "NoSuchService").is_none(), "{collection}");
        }
        assert!(find("Things", "GetPropertyValuesVTQ").is_some());
        assert!(find("DataShapes", "GetProperties").is_none());
        assert!(is_platform_shape("RootEntityList"));
        assert!(!is_platform_shape("Acme.Rows_DS"));
    }
}
