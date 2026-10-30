//! TIMESPAN and VARIANT property defaults, which a 10.2 server exports in another form than it
//! imports (observed 2026-10-30). A TIMESPAN default `start,end,period` is exported as
//! `{period=…, start=<ms>, end=<ms>}`, and a VARIANT default `abc` as
//! `{"value":"abc","baseType":"STRING"}`. Importing the exported form loses the value: the
//! TIMESPAN period resets or is garbled, and the VARIANT is wrapped a second time. So a file
//! holds the imported form, and a comparison takes both as one value.

use serde_json::Value;
use std::collections::BTreeMap;

/// `value` in the form an import reads, from either form; `None` when it is neither.
pub(crate) fn importable(base_type: &str, value: &str) -> Option<String> {
    match base_type {
        "TIMESPAN" => Timespan::parse(value).map(|span| span.text()),
        "VARIANT" => Some(variant_value(value).unwrap_or_else(|| value.to_string())),
        _ => None,
    }
}

/// `bytes` with each TIMESPAN and VARIANT property default in the form an import reads, so a
/// file holding what the server exported (a pull, `init --from-server`) deploys as it was.
pub(crate) fn importable_document(bytes: &[u8]) -> Vec<u8> {
    use crate::core::scan::{self, Kind};
    use crate::core::splice::{self, Edit};
    let Ok(tokens) = scan::tokenize(bytes) else {
        return bytes.to_vec();
    };
    let mut edits = Vec::new();
    for token in &tokens {
        if !matches!(token.kind, Kind::Start | Kind::Empty)
            || token.name.of(bytes) != b"PropertyDefinition"
        {
            continue;
        }
        let Ok(attributes) = scan::attributes(bytes, token) else {
            continue;
        };
        let get = |wanted: &[u8]| {
            attributes
                .iter()
                .find(|attribute| attribute.name.of(bytes) == wanted)
        };
        let Some(base_type) = get(b"baseType").map(|attribute| attribute.value.of(bytes)) else {
            continue;
        };
        let base_type = String::from_utf8_lossy(base_type);
        let Some(default) = get(b"aspect.defaultValue") else {
            continue;
        };
        let raw = String::from_utf8_lossy(default.value.of(bytes));
        let value = scan::decode_entities(&raw);
        let Some(wanted) = importable(&base_type, &value) else {
            continue;
        };
        if wanted != value {
            if let Ok(escaped) = scan::escape_attribute(&wanted) {
                edits.push(Edit::new(default.value, escaped.into_bytes()));
            }
        }
    }
    if edits.is_empty() {
        return bytes.to_vec();
    }
    splice::splice(bytes, &edits).unwrap_or_else(|_| bytes.to_vec())
}

/// The text of an exported variant, `{"value":"abc","baseType":"STRING"}`, of any base type.
fn variant_value(value: &str) -> Option<String> {
    let parsed: Value = serde_json::from_str(value).ok()?;
    let object = parsed.as_object()?;
    object.get("baseType")?.as_str()?;
    if object.len() != 2 {
        return None;
    }
    // A string is its text; a number, boolean or object is its JSON.
    Some(match object.get("value")? {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    })
}

/// A TIMESPAN: an optional start and end, and a period of six colon-separated numbers.
pub(crate) struct Timespan {
    start: Option<jiff::Timestamp>,
    end: Option<jiff::Timestamp>,
    period: String,
}

impl Timespan {
    /// `start,end,period` (start and end ISO timestamps or empty), or the exported
    /// `{period=…, start=<ms>, end=<ms>}`.
    pub(crate) fn parse(text: &str) -> Option<Timespan> {
        let (start, end, period) = if let Some(inner) = text
            .strip_prefix('{')
            .and_then(|rest| rest.strip_suffix('}'))
        {
            let mut fields = BTreeMap::new();
            for part in inner.split(", ") {
                let (key, value) = part.split_once('=')?;
                fields.insert(key, value);
            }
            let millis = |key: &str| -> Option<Option<jiff::Timestamp>> {
                match fields.get(key) {
                    None => Some(None),
                    Some(value) => value
                        .parse::<i64>()
                        .ok()
                        .and_then(|ms| jiff::Timestamp::from_millisecond(ms).ok())
                        .map(Some),
                }
            };
            (
                millis("start")?,
                millis("end")?,
                (*fields.get("period")?).to_string(),
            )
        } else {
            let parts: Vec<&str> = text.split(',').map(str::trim).collect();
            let [start, end, period] = parts.as_slice() else {
                return None;
            };
            let at = |value: &str| -> Option<Option<jiff::Timestamp>> {
                if value.is_empty() {
                    Some(None)
                } else {
                    value.parse().ok().map(Some)
                }
            };
            (at(start)?, at(end)?, (*period).to_string())
        };
        let fields: Vec<&str> = period.split(':').collect();
        (fields.len() == 6 && fields.iter().all(|field| field.parse::<u32>().is_ok()))
            .then_some(Timespan { start, end, period })
    }

    /// `start,end,period`, timestamps in UTC milliseconds: the stored value, and the default an
    /// import reads.
    pub(crate) fn text(&self) -> String {
        let at = |time: Option<jiff::Timestamp>| {
            time.map(|time| time.strftime("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
                .unwrap_or_default()
        };
        format!("{},{},{}", at(self.start), at(self.end), self.period)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_forms_read_as_the_imported_one() {
        // As a 10.2 server exported what it imported (2026-10-30).
        assert_eq!(
            importable("TIMESPAN", "{period=00:00:00:01:00:00}").unwrap(),
            ",,00:00:00:01:00:00"
        );
        assert_eq!(
            importable(
                "TIMESPAN",
                "{period=00:00:01:00:00:00, start=1767225600000, end=1769904000000}"
            )
            .unwrap(),
            "2026-01-01T00:00:00.000Z,2026-02-01T00:00:00.000Z,00:00:01:00:00:00"
        );
        assert_eq!(
            importable("TIMESPAN", "2026-01-01T00:00:00Z,,00:00:01:00:00:00").unwrap(),
            "2026-01-01T00:00:00.000Z,,00:00:01:00:00:00"
        );
        assert!(importable("TIMESPAN", ",,1").is_none());
        assert!(importable("TIMESPAN", "soon,,00:00:00:01:00:00").is_none());
        assert_eq!(
            importable("VARIANT", r#"{"value":"abc","baseType":"STRING"}"#).unwrap(),
            "abc"
        );
        assert_eq!(importable("VARIANT", "42").unwrap(), "42");
        assert_eq!(
            importable("VARIANT", r#"{"value":42,"baseType":"INTEGER"}"#).unwrap(),
            "42"
        );
        assert_eq!(
            importable("VARIANT", r#"{"value":true,"baseType":"BOOLEAN"}"#).unwrap(),
            "true"
        );
        assert_eq!(importable("STRING", "x"), None);
    }

    #[test]
    fn an_exported_document_is_rewritten_to_what_an_import_reads() {
        let exported = br#"<Thing><PropertyDefinition baseType="TIMESPAN" name="s" aspect.defaultValue="{period=00:00:00:01:00:00}"/><PropertyDefinition baseType="VARIANT" name="v" aspect.defaultValue="{&quot;value&quot;:&quot;a&amp;b&quot;,&quot;baseType&quot;:&quot;STRING&quot;}"/><PropertyDefinition baseType="STRING" name="t" aspect.defaultValue="{period=1}"/></Thing>"#;
        let rewritten = String::from_utf8(importable_document(exported)).unwrap();
        assert!(rewritten.contains(r#"name="s" aspect.defaultValue=",,00:00:00:01:00:00""#));
        assert!(rewritten.contains(r#"name="v" aspect.defaultValue="a&amp;b""#));
        assert!(rewritten.contains(r#"name="t" aspect.defaultValue="{period=1}""#));
        let imported = rewritten.as_bytes();
        assert_eq!(importable_document(imported), imported);
    }
}
