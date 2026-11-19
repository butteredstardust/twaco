//! Composer links from repository entities and a selected profile, without server access.

use super::{config::Solution, profile, scan, server::encode_path_segment, workspace};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Profile(#[from] profile::ProfileError),
    #[error("{0}")]
    Entity(#[from] workspace::WorkspaceError),
    #[error("{0}")]
    Invalid(String),
}

/// Resolve one solution entity and build its Composer link. No HTTP request is made.
pub fn entity_url(
    solution: &Solution,
    entity: &str,
    tab: Option<&str>,
    profile: &str,
) -> Result<String, Error> {
    let found = workspace::discover(solution);
    let entity = workspace::resolve(&found.entities, entity)?;
    let bytes = std::fs::read(&entity.path).map_err(|_| {
        Error::Invalid(format!(
            "cannot read entity {}; check its XML file",
            entity.info.name
        ))
    })?;
    let tokens = scan::tokenize(&bytes).map_err(|_| {
        Error::Invalid(format!(
            "cannot parse entity {}; check its XML file",
            entity.info.name
        ))
    })?;
    let element = tokens
        .iter()
        .filter(|t| matches!(t.kind, scan::Kind::Start | scan::Kind::Empty))
        .nth(2)
        .ok_or_else(|| Error::Invalid(format!("{} has no entity element", entity.info.name)))?;
    let kind = String::from_utf8_lossy(element.name.of(&bytes));
    has_details_page(&kind, &entity.info.name)?;
    let kind = composer_type(&entity.info.collection, &kind);
    let profile = profile::load(&solution.root, profile)?;
    link(&profile.url, kind, &entity.info.name, tab)
}

/// Types whose `#/modeler/details/<Type>~<name>` route does not open in Composer (checked on 10.2).
const NO_DETAILS_PAGE: &[&str] = &[
    "ExtensionPackage",
    "Log",
    "PersistenceProviderPackage",
    "QueueProviderPackage",
    "ScriptFunctionLibrary",
    "ThingPackage",
    "Widget",
];

fn has_details_page(element: &str, name: &str) -> Result<(), Error> {
    if NO_DETAILS_PAGE.contains(&element) {
        return Err(Error::Invalid(format!(
            "Composer has no details page for {element} entities such as {name}"
        )));
    }
    Ok(())
}

fn composer_type<'a>(collection: &str, element: &'a str) -> &'a str {
    match collection {
        "Things" | "DataTables" | "Streams" | "ValueStreams" | "Timers" | "Schedulers" => "Thing",
        "ThingTemplates" => "ThingTemplate",
        "ThingShapes" => "ThingShape",
        "MediaEntities" => "MediaEntity",
        "MCPNamespaces" => "MCPNamespace",
        "ModelTags" => "ModelTagVocabulary",
        "DataTags" => "DataTagVocabulary",
        // Gadgets are Mashups with aspect.mashupType="gadget".
        "MashupGadgets" => "Mashup",
        _ => element,
    }
}

fn link(base: &str, kind: &str, name: &str, tab: Option<&str>) -> Result<String, Error> {
    let tabs: &[&str] = match kind {
        "Thing" | "ThingTemplate" | "ThingShape" => &["attributes", "services"],
        "Mashup" => &["attributes", "design"],
        "DataShape" => &["attributes", "fields"],
        _ => &["attributes"],
    };
    let tab = tab.unwrap_or("attributes");
    if !tabs.contains(&tab) {
        return Err(Error::Invalid(format!(
            "--tab {tab:?} does not apply to {kind} {name}; use: {}",
            tabs.join(", ")
        )));
    }
    let mut base = url::Url::parse(base).map_err(|_| {
        Error::Invalid("profile URL is invalid; set an absolute http or https URL".into())
    })?;
    if !matches!(base.scheme(), "http" | "https") || base.host_str().is_none() {
        return Err(Error::Invalid(
            "profile URL needs an http or https server address".into(),
        ));
    }
    // Profiles may embed credentials; a printable link must never carry them.
    let _ = base.set_username("");
    let _ = base.set_password(None);
    base.set_query(None);
    base.set_fragment(None);
    let path = base.path().trim_end_matches('/');
    let path = if path.is_empty() {
        "/Thingworx".to_string()
    } else {
        path.to_string()
    };
    base.set_path(&format!("{path}/Composer/index.html"));
    Ok(format!(
        "{base}#/modeler/details/{}~{}/{tab}",
        encode_path_segment(kind),
        encode_path_segment(name).replace('~', "%7E")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composer_types_cover_field_verified_routes_and_thing_collections() {
        for (collection, element, expected) in [
            ("Things", "Thing", "Thing"),
            ("ValueStreams", "Thing", "Thing"),
            ("Timers", "Timer", "Thing"),
            ("Schedulers", "Scheduler", "Thing"),
            ("DataTables", "DataTable", "Thing"),
            ("Streams", "Stream", "Thing"),
            ("ThingTemplates", "ThingTemplate", "ThingTemplate"),
            ("ThingShapes", "ThingShape", "ThingShape"),
            ("DataShapes", "DataShape", "DataShape"),
            ("Mashups", "Mashup", "Mashup"),
            ("Menus", "Menu", "Menu"),
            ("MediaEntities", "Media", "MediaEntity"),
            ("ApplicationKeys", "ApplicationKey", "ApplicationKey"),
            ("MCPNamespaces", "MCPNamespace", "MCPNamespace"),
            ("QueueProviders", "QueueProvider", "QueueProvider"),
            ("Groups", "Group", "Group"),
            ("Roles", "Role", "Role"),
            ("PermissionGroups", "PermissionGroup", "PermissionGroup"),
            ("Projects", "Project", "Project"),
            ("ModelTags", "ModelTag", "ModelTagVocabulary"),
            ("DataTags", "DataTag", "DataTagVocabulary"),
            ("MashupGadgets", "MashupGadget", "Mashup"),
            ("ThingGroups", "ThingGroup", "ThingGroup"),
            (
                "NotificationDefinitions",
                "NotificationDefinition",
                "NotificationDefinition",
            ),
            ("DirectoryServices", "DirectoryService", "DirectoryService"),
            ("Subsystems", "Subsystem", "Subsystem"),
        ] {
            assert_eq!(composer_type(collection, element), expected);
        }
    }

    #[test]
    fn types_without_a_details_page_are_refused() {
        for element in NO_DETAILS_PAGE {
            let error = has_details_page(element, "Acme.X").unwrap_err().to_string();
            assert!(error.contains("no details page"), "{error}");
        }
        assert!(has_details_page("Thing", "Acme.X").is_ok());
        assert!(has_details_page("DirectoryService", "Acme.X").is_ok());
    }

    #[test]
    fn tabs_are_type_specific_and_attributes_is_the_default() {
        for (kind, allowed) in [
            ("Thing", "services"),
            ("ThingTemplate", "services"),
            ("ThingShape", "services"),
            ("Mashup", "design"),
            ("DataShape", "fields"),
        ] {
            assert!(link(
                "https://example.invalid",
                kind,
                "Acme.Entity",
                Some(allowed)
            )
            .is_ok());
            assert!(link("https://example.invalid", kind, "Acme.Entity", None)
                .unwrap()
                .ends_with("/attributes"));
        }
        let error = link(
            "https://example.invalid",
            "Menu",
            "Acme.Menu",
            Some("design"),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("--tab"));
        assert!(error.ends_with("use: attributes"));
        let error = link(
            "https://example.invalid",
            "DataShape",
            "Acme.Rows",
            Some("services"),
        )
        .unwrap_err()
        .to_string();
        assert!(error.ends_with("use: attributes, fields"));
    }

    #[test]
    fn explicit_application_context_is_preserved() {
        for context in ["/twx", "/proxy/twx", "/Thingworx"] {
            for trailing in ["", "/"] {
                let base = format!("https://example.invalid{context}{trailing}");
                assert!(link(&base, "Thing", "Acme.T", None)
                    .unwrap()
                    .starts_with(&format!("https://example.invalid{context}/Composer/")));
            }
        }
        for base in ["https://example.invalid", "https://example.invalid/"] {
            assert!(link(base, "Thing", "Acme.T", None)
                .unwrap()
                .starts_with("https://example.invalid/Thingworx/Composer/"));
        }
    }

    #[test]
    fn names_are_encoded_and_profile_credentials_are_removed() {
        assert_eq!(link("https://alice:secret@example.invalid/proxy/Thingworx/?key=secret#secret", "Thing", "Acme.Caf\u{e9} &?#/~%", None).unwrap(),
            "https://example.invalid/proxy/Thingworx/Composer/index.html#/modeler/details/Thing~Acme.Caf%C3%A9%20%26%3F%23%2F%7E%25/attributes");
        assert!(
            link("https://example.invalid/", "Thing", "Acme.Thing", None)
                .unwrap()
                .starts_with("https://example.invalid/Thingworx/Composer/")
        );
        assert!(link("file:///secret", "Thing", "Acme.Thing", None).is_err());
    }
}
