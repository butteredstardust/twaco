use super::super::schema::validate_arguments;
use schemars::generate::SchemaSettings;
use schemars::{JsonSchema, Schema};
use serde::de::{DeserializeOwned, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::ops::Deref;

/// An argument that may be absent, but may not be JSON `null`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Absent<T>(Option<T>);

impl<T> Default for Absent<T> {
    fn default() -> Self {
        Self(None)
    }
}

impl<T> Absent<T> {
    pub(crate) fn as_ref(&self) -> Option<&T> {
        self.0.as_ref()
    }

    pub(crate) fn is_absent(&self) -> bool {
        self.0.is_none()
    }
}

impl<T> Absent<Vec<T>> {
    /// The items, none when the argument is absent.
    pub(crate) fn items(&self) -> &[T] {
        self.0.as_deref().unwrap_or_default()
    }
}

impl<T> Deref for Absent<T> {
    type Target = Option<T>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Absent<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        T::deserialize(deserializer).map(|value| Self(Some(value)))
    }
}

impl<T: Serialize> Serialize for Absent<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<T: JsonSchema> JsonSchema for Absent<T> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        T::schema_name()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> Schema {
        T::json_schema(generator)
    }
}

/// Generate the conservative, inlined schema advertised by MCP.
pub(crate) fn schema<T: JsonSchema>() -> Value {
    let settings = SchemaSettings::draft2020_12().with(|settings| {
        settings.inline_subschemas = true;
    });
    let schema = settings.into_generator().into_root_schema_for::<T>();
    let mut value = serde_json::to_value(schema).expect("schemas serialise");
    remove_generated_keywords(&mut value);
    value
}

fn remove_generated_keywords(value: &mut Value) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    for key in ["$schema", "$defs", "$ref", "title"] {
        object.remove(key);
    }
    object.remove("format");
    for value in object.values_mut() {
        remove_generated_keywords(value);
    }
    if let Some(items) = object.get_mut("items") {
        remove_generated_keywords(items);
    }
}

/// Check arguments against a request's published schema, so a client sees the messages it
/// always has, then read them into the request.
pub(crate) fn parse<T: DeserializeOwned>(schema: &Value, arguments: &Value) -> Result<T, String> {
    validate_arguments(schema, arguments)?;
    serde_json::from_value(arguments.clone()).map_err(|error| format!("invalid arguments: {error}"))
}

// A tool that takes no arguments. Not a doc comment: that would become the schema's description.
#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NoArguments {}

/// The schema of a free-form JSON object: any members, any values.
pub(crate) fn free_object(_: &mut schemars::SchemaGenerator) -> Schema {
    schemars::json_schema!({ "type": "object" })
}

pub(crate) fn default_true() -> bool {
    true
}

pub(crate) fn default_profile() -> String {
    "default".to_string()
}
