//! Validated addresses for entities and service calls.

use super::server::{encode_path_segment, ServerError};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// Why text cannot identify one URL path segment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyError {
    text: String,
    why: &'static str,
}

impl KeyError {
    fn new(text: impl Into<String>, why: &'static str) -> Self {
        Self {
            text: text.into(),
            why,
        }
    }
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.text, self.why)
    }
}

impl std::error::Error for KeyError {}

impl From<KeyError> for ServerError {
    fn from(error: KeyError) -> Self {
        ServerError::InvalidUrl(format!(
            "call target {:?} must be a Thing name or Collection/Name",
            error.text
        ))
    }
}

fn segment(text: &str) -> Result<(), KeyError> {
    let why = if text.is_empty() {
        Some("a segment is empty")
    } else if text == "." || text == ".." {
        Some("a segment is . or ..")
    } else if text.contains('/') {
        Some("a segment contains /")
    } else {
        None
    };
    why.map_or(Ok(()), |why| Err(KeyError::new(text, why)))
}

/// A collection and entity name. Both segments are non-empty and cannot contain a slash or be
/// `.` or `..`; malformed addresses are refused rather than being resolved as another route.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntityKey {
    collection: String,
    name: String,
}

impl EntityKey {
    pub fn new(collection: impl Into<String>, name: impl Into<String>) -> Result<Self, KeyError> {
        let collection = collection.into();
        let name = name.into();
        segment(&collection)?;
        segment(&name)?;
        Ok(Self { collection, name })
    }

    pub fn parse(text: &str) -> Result<Self, KeyError> {
        let Some((collection, name)) = text.split_once('/') else {
            return Err(KeyError::new(text, "an entity key needs Collection/Name"));
        };
        if name.contains('/') {
            return Err(KeyError::new(
                text,
                "an entity key has more than two segments",
            ));
        }
        Self::new(collection, name)
            .map_err(|_| KeyError::new(text, "an entity key has an invalid segment"))
    }

    pub fn collection(&self) -> &str {
        &self.collection
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn url_path(&self) -> String {
        format!(
            "{}/{}",
            encode_path_segment(&self.collection),
            encode_path_segment(&self.name)
        )
    }
}

impl fmt::Display for EntityKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.collection, self.name)
    }
}

impl Serialize for EntityKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for EntityKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// An address accepted by ThingWorx's service route: a bare Thing or a qualified entity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServiceTarget {
    Thing(String),
    Entity(EntityKey),
}

impl ServiceTarget {
    pub fn parse(text: &str) -> Result<Self, KeyError> {
        if text.contains('/') {
            EntityKey::parse(text).map(Self::Entity)
        } else {
            segment(text)?;
            Ok(Self::Thing(text.to_string()))
        }
    }

    pub fn entity(
        collection: impl Into<String>,
        name: impl Into<String>,
    ) -> Result<Self, KeyError> {
        let collection = collection.into();
        let name = name.into();
        EntityKey::new(&collection, &name)
            .map(Self::Entity)
            .map_err(|_| {
                KeyError::new(
                    format!("{collection}/{name}"),
                    "an entity key has an invalid segment",
                )
            })
    }

    /// Name a fixed platform entity whose literal address is validated with the binary.
    pub fn platform(collection: &str, name: &str) -> Self {
        Self::entity(collection, name).expect("platform target literals are valid")
    }

    pub fn url_path(&self) -> String {
        match self {
            ServiceTarget::Thing(name) => format!("Things/{}", encode_path_segment(name)),
            ServiceTarget::Entity(key) => key.url_path(),
        }
    }
}

impl fmt::Display for ServiceTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServiceTarget::Thing(name) => f.write_str(name),
            ServiceTarget::Entity(key) => key.fmt(f),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{EntityKey, ServiceTarget};
    use crate::core::server::ServerError;

    #[test]
    fn entity_key_parses_displays_and_refuses_invalid_addresses() {
        let key = EntityKey::parse("Widgets/Example").unwrap();
        assert_eq!(key.to_string(), "Widgets/Example");
        for bad in [
            "",
            ".",
            "..",
            "Widgets",
            "Widgets/",
            "/Example",
            "./Example",
            "Widgets/../Example",
            "A/B/C",
        ] {
            assert!(EntityKey::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn service_targets_keep_bare_names_and_encode_complete_segments() {
        assert_eq!(
            ServiceTarget::parse("A Thing").unwrap().to_string(),
            "A Thing"
        );
        assert_eq!(
            ServiceTarget::parse("Resources/SourceControlFunctions")
                .unwrap()
                .to_string(),
            "Resources/SourceControlFunctions"
        );
        for bad in ["a/../b", "/x", "x/", "..", "", "a//b"] {
            assert!(ServiceTarget::parse(bad).is_err(), "{bad:?}");
        }
        assert_eq!(
            EntityKey::new("Some Collection", "%#?é")
                .unwrap()
                .url_path(),
            "Some%20Collection/%25%23%3F%C3%A9"
        );
        assert_eq!(
            ServiceTarget::parse("A Thing").unwrap().url_path(),
            "Things/A%20Thing"
        );
    }

    #[test]
    fn entity_keys_serialize_as_their_address_and_bad_targets_keep_the_old_error() {
        let key = EntityKey::new("Things", "One").unwrap();
        assert_eq!(serde_json::to_string(&key).unwrap(), "\"Things/One\"");
        assert_eq!(
            serde_json::from_str::<EntityKey>("\"Things/One\"").unwrap(),
            key
        );
        assert!(serde_json::from_str::<EntityKey>("\"Things//One\"").is_err());
        let error: ServerError = ServiceTarget::parse("Things/../Users").unwrap_err().into();
        assert_eq!(error.to_string(), "invalid server URL: call target \"Things/../Users\" must be a Thing name or Collection/Name");
    }
}
