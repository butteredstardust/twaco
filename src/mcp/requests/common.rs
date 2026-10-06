use schemars::generate::SchemaSettings;
use schemars::{JsonSchema, Schema};
use serde::de::{Deserialize, DeserializeOwned, Deserializer};
use serde::Serialize;
use serde_json::{Map, Value};
use std::ops::Deref;

/// An argument that may be absent, but may not be JSON `null`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Absent<T>(Option<T>);

impl<T> Absent<T> {
    pub(crate) fn as_ref(&self) -> Option<&T> {
        self.0.as_ref()
    }

    pub(crate) fn is_absent(&self) -> bool {
        self.0.is_none()
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

/// Turn a serde failure into the tool errors clients already receive.
pub(crate) fn parse<T>(arguments: &Value) -> Result<T, String>
where
    T: DeserializeOwned + JsonSchema,
{
    let schema = schema::<T>();
    if let Some(error) = validation_error(&schema, arguments) {
        return Err(error);
    }
    match serde_json::from_value(arguments.clone()) {
        Ok(request) => Ok(request),
        Err(_) => Err(normalise_error(&schema, arguments)),
    }
}

fn normalise_error(schema: &Value, value: &Value) -> String {
    validation_error(schema, value).unwrap_or_else(|| "invalid arguments".to_string())
}

fn validation_error(schema: &Value, value: &Value) -> Option<String> {
    if !value.is_object() {
        return Some("`arguments` must be a JSON object".to_string());
    }
    validate_value(schema, value, "")
}

fn validate_value(schema: &Value, value: &Value, path: &str) -> Option<String> {
    let fits = match schema["type"].as_str() {
        Some("string") => value.is_string(),
        Some("boolean") => value.is_boolean(),
        Some("integer") => value
            .as_u64()
            .is_some_and(|number| schema["minimum"].as_u64().is_none_or(|min| number >= min)),
        Some("object") => value.is_object(),
        Some("array") => value.is_array(),
        _ => true,
    };
    if !fits {
        let wanted = match schema["type"].as_str() {
            Some("array") if array_of_strings(schema) => "an array of strings".to_string(),
            Some("array") => "an array".to_string(),
            Some("integer") => format!(
                "an integer of at least {}",
                schema["minimum"].as_u64().unwrap_or(0)
            ),
            Some(other) => format!("a {other}"),
            None => "something else".to_string(),
        };
        return Some(format!("`{path}` must be {wanted}, not {value}"));
    }
    if let Some(allowed) = schema["enum"].as_array() {
        if !allowed.contains(value) {
            return Some(format!(
                "`{path}` must be one of {}, not {value}",
                Value::Array(allowed.clone())
            ));
        }
    }
    if let Some(given) = value.as_object() {
        let empty = Map::new();
        let properties = schema["properties"].as_object().unwrap_or(&empty);
        for name in schema["required"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !given.contains_key(name) {
                return Some(format!("`{}` is required", property_path(path, name)));
            }
        }
        for (name, value) in given {
            let name_path = property_path(path, name);
            match properties.get(name) {
                Some(property) => {
                    if let Some(error) = validate_value(property, value, &name_path) {
                        return Some(error);
                    }
                }
                None if schema["additionalProperties"] == false => {
                    if path.is_empty() {
                        let mut known: Vec<&str> = properties.keys().map(String::as_str).collect();
                        known.sort();
                        return Some(format!(
                            "this tool takes no argument `{name}` (it takes: {})",
                            known.join(", ")
                        ));
                    }
                    return Some(format!("`{name_path}` is not allowed"));
                }
                None => {}
            }
        }
    }
    if let Some(items) = value.as_array() {
        if schema["items"].is_object() {
            for (index, item) in items.iter().enumerate() {
                if let Some(error) =
                    validate_value(&schema["items"], item, &format!("{path}[{index}]"))
                {
                    return Some(error);
                }
            }
        }
    }
    None
}

fn property_path(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}.{name}")
    }
}

fn array_of_strings(schema: &Value) -> bool {
    schema["items"].as_object().is_some_and(|items| {
        items.len() == 1 && items.get("type").and_then(Value::as_str) == Some("string")
    })
}
