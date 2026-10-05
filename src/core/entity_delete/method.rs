use super::super::bundle::COLLECTION_ORDER;
use serde::Serialize;
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Method {
    Service { service: String },
    RestDelete,
    Composer,
}

impl Serialize for Method {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Method::Service { service } => write!(f, "EntityServices.{service}"),
            Method::RestDelete => f.write_str("REST DELETE"),
            Method::Composer => f.write_str("delete it in Composer"),
        }
    }
}

/// The delete route is an explicit allow-list. Verified ThingWorx EntityServices metadata
/// reports one STRING input named `name` for every listed service: DeleteThing,
/// DeleteThingTemplate, DeleteThingShape, DeleteMediaEntity, DeleteGroup, DeleteOrganization,
/// DeleteProject and DeleteUser. Collections listed as REST use Composer's verified route.
pub fn method_for(collection: &str) -> Option<Method> {
    let service = match collection {
        "Things" => Some("DeleteThing"),
        "ThingTemplates" => Some("DeleteThingTemplate"),
        "ThingShapes" => Some("DeleteThingShape"),
        "MediaEntities" => Some("DeleteMediaEntity"),
        "Groups" => Some("DeleteGroup"),
        "Organizations" => Some("DeleteOrganization"),
        "Projects" => Some("DeleteProject"),
        "Users" => Some("DeleteUser"),
        _ => None,
    };
    if let Some(service) = service {
        return Some(Method::Service {
            service: service.to_string(),
        });
    }
    match collection {
        "ApplicationKeys"
        | "Dashboards"
        | "DataShapes"
        | "DataTables"
        | "Localizations"
        | "LocalizationTables"
        | "MashupGadgets"
        | "Mashups"
        | "Menus"
        | "ModelTags"
        | "Networks"
        | "NotificationContents"
        | "NotificationDefinitions"
        | "PersistenceProviders"
        | "Schedulers"
        | "StateDefinitions"
        | "Streams"
        | "StyleDefinitions"
        | "StyleThemes"
        | "ThingGroups"
        | "Timers"
        | "ValueStreams"
        | "Widgets"
        | "MCPNamespaces"
        | "AIAgents" => Some(Method::RestDelete),
        _ => None,
    }
}

pub(super) fn known_collections() -> Vec<&'static str> {
    let mut collections: Vec<&str> = COLLECTION_ORDER
        .iter()
        .copied()
        .filter(|collection| method_for(collection).is_some())
        .collect();
    for extra in [
        "DataTables",
        "Localizations",
        "MashupGadgets",
        "Schedulers",
        "Streams",
        "Timers",
        "ValueStreams",
    ] {
        if !collections.contains(&extra) {
            collections.push(extra);
        }
    }
    collections
}
