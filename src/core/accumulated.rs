//! The `accumulatedDataShape` JSON which ThingWorx stores on a DataTable or Stream.
//!
//! These are the merge rules verified against ThingWorx 10.1: existing entries retain their
//! ordinal, absent fields are marked deleted, present fields are retyped (and undeleted), and
//! new fields are appended in case-insensitive name order.  Base DataShape fields never count:
//! `fields` is only the imported DataShape's own fields.

use super::datashape::{Aspect, Field};
use serde_json::{Map, Value};

/// Merge the server's accumulated shape with a DataShape's own fields.  Invalid and incomplete
/// accumulated shapes are deliberately left alone because the server behaviour for them was not
/// verified.
pub(crate) fn merge(old: &str, fields: &[Field]) -> Option<String> {
    let value: Value = serde_json::from_str(old).ok()?;
    let definitions = value.get("fieldDefinitions")?.as_object()?;
    let mut entries: Vec<(String, Value)> = definitions
        .iter()
        .map(|(name, field)| (name.clone(), field.clone()))
        .collect();
    let mut maximum = -1i64;
    for (_, field) in &entries {
        if let Some(ordinal) = field.get("ordinal").and_then(Value::as_i64) {
            maximum = maximum.max(ordinal);
        }
    }

    for (name, field) in &mut entries {
        let field = field.as_object_mut()?;
        if let Some(wanted) = fields.iter().find(|wanted| wanted.name == *name) {
            field.insert("baseType".into(), Value::String(wanted.base_type.clone()));
            if let Some(aspects) = field.get_mut("aspects").and_then(Value::as_object_mut) {
                if aspects.contains_key("isDeleted") {
                    aspects.insert("isDeleted".into(), Value::Bool(false));
                }
            }
        } else {
            let aspects = aspects_mut(field)?;
            aspects.insert("isDeleted".into(), Value::Bool(true));
        }
    }
    let known: std::collections::BTreeSet<&str> =
        entries.iter().map(|(name, _)| name.as_str()).collect();
    let mut additions: Vec<&Field> = fields
        .iter()
        .filter(|field| !known.contains(field.name.as_str()))
        .collect();
    additions
        .sort_by(|a, b| (a.name.to_lowercase(), &a.name).cmp(&(b.name.to_lowercase(), &b.name)));
    for field in additions {
        maximum += 1;
        entries.push((field.name.clone(), fresh_value(field, maximum)));
    }
    // The server fills its HashMap in ordinal order, which decides the order of keys that share
    // a bucket; the old text's order is only the iteration order of a smaller map.
    entries.sort_by_key(|(_, field)| field.get("ordinal").and_then(Value::as_i64));

    let rendered = format!(
        "{{\"fieldDefinitions\":{}}}",
        java_hash_map(
            entries
                .into_iter()
                .map(|(name, field)| (name, render_field(&field)))
                .collect()
        )
    );
    (serde_json::from_str::<Value>(&rendered).ok()? != value).then_some(rendered)
}

fn aspects_mut(field: &mut Map<String, Value>) -> Option<&mut Map<String, Value>> {
    if !field.contains_key("aspects") {
        field.insert("aspects".into(), Value::Object(Map::new()));
    }
    field.get_mut("aspects")?.as_object_mut()
}

/// The accumulated shape used for a newly-created DataTable or Stream.
pub(crate) fn accumulated_json(fields: &[Field]) -> String {
    let mut sorted: Vec<&Field> = fields.iter().collect();
    sorted.sort_by(|a, b| (a.name.to_lowercase(), &a.name).cmp(&(b.name.to_lowercase(), &b.name)));
    let entries = sorted
        .into_iter()
        .enumerate()
        .map(|(ordinal, field)| {
            (
                field.name.clone(),
                render_field(&fresh_value(field, ordinal as i64)),
            )
        })
        .collect();
    format!("{{\"fieldDefinitions\":{}}}", java_hash_map(entries))
}

fn fresh_value(field: &Field, ordinal: i64) -> Value {
    let aspects: Map<String, Value> = field
        .aspects
        .iter()
        .map(|(key, value)| (key.clone(), aspect_value(key, value, &field.base_type)))
        .collect();
    let mut out = Map::new();
    out.insert("name".into(), Value::String(field.name.clone()));
    out.insert("aspects".into(), Value::Object(aspects));
    out.insert(
        "description".into(),
        Value::String(field.description.clone()),
    );
    out.insert("baseType".into(), Value::String(field.base_type.clone()));
    out.insert("ordinal".into(), Value::Number(ordinal.into()));
    Value::Object(out)
}

fn aspect_value(key: &str, value: &Aspect, base_type: &str) -> Value {
    match value {
        Aspect::Bool(flag) => Value::Bool(*flag),
        Aspect::Text(text) if key == "defaultValue" => match base_type {
            "NUMBER" => text
                .parse::<f64>()
                .ok()
                .filter(|n| n.is_finite())
                .and_then(serde_json::Number::from_f64)
                .map_or_else(|| Value::String(text.clone()), Value::Number),
            "INTEGER" | "LONG" => text.parse::<i64>().map_or_else(
                |_| Value::String(text.clone()),
                |number| Value::Number(number.into()),
            ),
            _ => Value::String(text.clone()),
        },
        Aspect::Text(text) => Value::String(text.clone()),
    }
}

fn render_field(value: &Value) -> String {
    let Some(field) = value.as_object() else {
        return value.to_string();
    };
    let mut parts = Vec::new();
    for key in ["name", "aspects", "description", "baseType", "ordinal"] {
        if let Some(value) = field.get(key) {
            let rendered = if key == "aspects" {
                value
                    .as_object()
                    .map(|aspects| {
                        java_hash_map(
                            aspects
                                .iter()
                                .map(|(key, value)| (key.clone(), value.to_string()))
                                .collect(),
                        )
                    })
                    .unwrap_or_else(|| value.to_string())
            } else {
                value.to_string()
            };
            parts.push(format!("{}:{rendered}", json_string(key)));
        }
    }
    format!("{{{}}}", parts.join(","))
}

fn json_string(text: &str) -> String {
    Value::String(text.to_string()).to_string()
}

/// A JSON object in `java.util.HashMap<String, _>` iteration order after insertion.
pub(crate) fn java_hash_map(entries: Vec<(String, String)>) -> String {
    let mut capacity = 16usize;
    while entries.len() * 4 > capacity * 3 {
        capacity *= 2;
    }
    let bucket = |key: &str| {
        let hash = key.encode_utf16().fold(0u32, |hash, unit| {
            hash.wrapping_mul(31).wrapping_add(u32::from(unit))
        });
        ((hash ^ (hash >> 16)) as usize) & (capacity - 1)
    };
    let mut ordered: Vec<_> = entries
        .iter()
        .enumerate()
        .map(|(at, entry)| (bucket(&entry.0), at, entry))
        .collect();
    ordered.sort_by_key(|(bucket, at, _)| (*bucket, *at));
    format!(
        "{{{}}}",
        ordered
            .iter()
            .map(|(_, _, (key, value))| format!("{}:{value}", json_string(key)))
            .collect::<Vec<_>>()
            .join(",")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_101_rounds_are_byte_for_byte_and_settle() {
        let rounds: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/accumulated/live_rounds.json"
        ))
        .unwrap();
        for round in rounds["rounds"].as_array().unwrap() {
            let fields =
                crate::core::datashape::from_sidecar(&round["fields"].to_string()).unwrap();
            let actual = merge(round["before"].as_str().unwrap(), &fields).unwrap();
            assert_eq!(
                actual,
                round["server"].as_str().unwrap(),
                "{}",
                round["what"]
            );
            assert_eq!(merge(&actual, &fields), None);
        }
    }

    #[test]
    fn malformed_or_unshaped_values_are_left_alone() {
        assert_eq!(merge("", &[]), None);
        assert_eq!(merge("null", &[]), None);
        assert_eq!(merge("{}", &[]), None);
    }

    #[test]
    fn equal_json_keeps_the_hand_written_text() {
        let old = r#"{ "fieldDefinitions" : { } }"#;
        assert_eq!(merge(old, &[]), None);
        // Insertion order and ordinals from 1, as a hand-written table in a real repository has.
        let old = r#"{"fieldDefinitions":{"uuid":{"name":"uuid","aspects":{"isPrimaryKey":true},"description":"","baseType":"STRING","ordinal":1},"Answer":{"name":"Answer","aspects":{"isPrimaryKey":false},"description":"","baseType":"STRING","ordinal":2}}}"#;
        let fields = [field("uuid", "STRING"), field("Answer", "STRING")];
        assert_eq!(merge(old, &fields), None);
    }

    fn field(name: &str, base_type: &str) -> Field {
        Field {
            name: name.into(),
            base_type: base_type.into(),
            ordinal: String::new(),
            description: String::new(),
            aspects: [("isPrimaryKey".to_string(), Aspect::Bool(false))]
                .into_iter()
                .collect(),
        }
    }

    #[test]
    fn keys_sharing_a_bucket_go_in_ordinal_order() {
        // "al" and "bm" share a bucket of a 16-slot HashMap; the old text lists bm first.
        let old = r#"{"fieldDefinitions":{"bm":{"name":"bm","aspects":{"isPrimaryKey":false},"description":"","baseType":"STRING","ordinal":1},"al":{"name":"al","aspects":{"isPrimaryKey":false},"description":"","baseType":"STRING","ordinal":0}}}"#;
        let merged = merge(
            old,
            &[
                field("al", "STRING"),
                field("bm", "STRING"),
                field("zz", "STRING"),
            ],
        )
        .unwrap();
        assert!(
            merged.find("\"al\":").unwrap() < merged.find("\"bm\":").unwrap(),
            "{merged}"
        );
    }

    #[test]
    fn new_ordinals_continue_from_the_highest_and_a_returning_field_keeps_its_own() {
        let old = r#"{"fieldDefinitions":{"a":{"name":"a","aspects":{"isPrimaryKey":false},"description":"","baseType":"STRING","ordinal":1},"b":{"name":"b","aspects":{"isDeleted":true,"isPrimaryKey":false},"description":"","baseType":"STRING","ordinal":5}}}"#;
        let merged = merge(
            old,
            &[
                field("b", "INTEGER"),
                field("Z", "STRING"),
                field("c", "STRING"),
            ],
        )
        .unwrap();
        let value: Value = serde_json::from_str(&merged).unwrap();
        let definitions = &value["fieldDefinitions"];
        assert_eq!(definitions["a"]["aspects"]["isDeleted"], Value::Bool(true));
        assert_eq!(definitions["a"]["ordinal"], 1);
        assert_eq!(definitions["b"]["aspects"]["isDeleted"], Value::Bool(false));
        assert_eq!(definitions["b"]["baseType"], "INTEGER");
        assert_eq!(definitions["b"]["ordinal"], 5);
        assert_eq!(definitions["c"]["ordinal"], 6);
        assert_eq!(definitions["Z"]["ordinal"], 7);
    }
}
