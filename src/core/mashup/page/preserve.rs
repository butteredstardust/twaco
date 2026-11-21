use super::merge::key;
use serde_json::Value;
use std::ops::Range;

struct Child {
    key: Option<String>,
    prefix: Range<usize>,
    value: Range<usize>,
}

fn skip(text: &str, at: &mut usize) {
    while text
        .as_bytes()
        .get(*at)
        .is_some_and(u8::is_ascii_whitespace)
    {
        *at += 1;
    }
}

fn value(text: &str, at: &mut usize) -> (Value, Range<usize>) {
    skip(text, at);
    let start = *at;
    let mut stream = serde_json::Deserializer::from_str(&text[start..]).into_iter::<Value>();
    let parsed = stream.next().expect("JSON value").expect("validated JSON");
    *at += stream.byte_offset();
    (parsed, start..*at)
}

fn children(text: &str, object: bool) -> Vec<Child> {
    let mut at = 1;
    let mut out = Vec::new();
    loop {
        skip(text, &mut at);
        if matches!(text.as_bytes().get(at), Some(b'}' | b']')) {
            break;
        }
        let start = at;
        let key = if object {
            let (key, _) = value(text, &mut at);
            skip(text, &mut at);
            at += 1;
            Some(key.as_str().expect("object key").into())
        } else {
            None
        };
        skip(text, &mut at);
        let prefix = start..at;
        let (_, range) = value(text, &mut at);
        out.push(Child {
            key,
            prefix,
            value: range,
        });
        skip(text, &mut at);
        if text.as_bytes().get(at) == Some(&b',') {
            at += 1;
        }
    }
    out
}

/// Keep unchanged JSON values and their spelling; splice only changed child spans when possible.
pub(super) fn render(raw: &str, wanted: &Value) -> String {
    let current: Value = serde_json::from_str(raw).expect("validated JSON");
    if &current == wanted {
        return raw.into();
    }
    let leading = raw.len() - raw.trim_start().len();
    let text = raw.trim();
    let (object, items): (bool, Vec<(Option<String>, &Value)>) = match wanted {
        Value::Object(m) if current.is_object() => {
            (true, m.iter().map(|(k, v)| (Some(k.clone()), v)).collect())
        }
        Value::Array(a) if current.is_array() => (false, a.iter().map(|v| (key(v), v)).collect()),
        _ => return serde_json::to_string(wanted).expect("JSON"),
    };
    let existing = children(text, object);
    let matches = |i: usize, k: &Option<String>| -> Option<usize> {
        if object {
            existing.iter().position(|c| &c.key == k)
        } else if let Some(k) = k {
            current
                .as_array()
                .expect("array")
                .iter()
                .position(|v| key(v).as_ref() == Some(k))
        } else {
            (i < existing.len()).then_some(i)
        }
    };
    let same_shape = items.len() == existing.len()
        && items
            .iter()
            .enumerate()
            .all(|(i, (k, _))| matches(i, k) == Some(i));
    if same_shape {
        let mut out = raw.to_string();
        for (i, (_, v)) in items.iter().enumerate().rev() {
            let span = &existing[i].value;
            let replacement = render(&text[span.clone()], v);
            out.replace_range(leading + span.start..leading + span.end, &replacement);
        }
        return out;
    }
    let rendered: Vec<_> = items
        .iter()
        .enumerate()
        .map(|(i, (k, v))| {
            if let Some(index) = matches(i, k) {
                let child = &existing[index];
                let prefix = if object {
                    &text[child.prefix.clone()]
                } else {
                    ""
                };
                format!("{prefix}{}", render(&text[child.value.clone()], v))
            } else if let Some(k) = k.as_ref().filter(|_| object) {
                format!(
                    "{}: {}",
                    serde_json::to_string(k).expect("key"),
                    serde_json::to_string(v).expect("value")
                )
            } else {
                serde_json::to_string(v).expect("value")
            }
        })
        .collect();
    format!(
        "{}{}{}",
        if object { "{" } else { "[" },
        rendered.join(","),
        if object { "}" } else { "]" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn unchanged_spelling_and_unmanaged_widget_bytes_survive() {
        let raw =
            "{ \"managed\" : 1, \"Acme-extra\"  : {\"number\":1.00, \"escaped\":\"a\\u0062\"}}\n";
        let result = render(
            raw,
            &json!({"managed":2,"Acme-extra":{"number":1.0,"escaped":"ab"}}),
        );
        assert_eq!(result, raw.replacen(": 1,", ": 2,", 1));
        let raw = "[ { \"Id\":\"Acme-a\",\"n\":1.00 } ]";
        let result = render(raw, &json!([{"Id":"Acme-a","n":1.0},{"Id":"Acme-b"}]));
        assert!(result.contains("{ \"Id\":\"Acme-a\",\"n\":1.00 }"));
    }
}
