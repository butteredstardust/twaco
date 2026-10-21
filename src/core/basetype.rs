//! Literal values accepted by ThingWorx base types.

use std::str::FromStr;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Mapped {
    Same,
    Rewritten(String),
    Unmappable(&'static str),
}

/// A scalar literal as a script writes it: a string's raw text between its quotes, or a number's
/// text. An object or a non-literal expression is not mapped.
pub(crate) enum Literal<'a> {
    String(&'a str),
    Number(&'a str),
    Bool(bool),
    Null,
}

pub(crate) fn map_literal(literal: &Literal<'_>, to: &str) -> Mapped {
    if matches!(literal, Literal::Null) {
        return Mapped::Same;
    }
    let to = to.to_ascii_uppercase();
    if matches!(
        to.as_str(),
        "INFOTABLE" | "LOCATION" | "XML" | "BLOB" | "TAGS"
    ) || to.starts_with("VEC")
    {
        return Mapped::Unmappable("no literal of this type");
    }
    if matches!(to.as_str(), "JSON" | "QUERY") && matches!(literal, Literal::String(_)) {
        return Mapped::Unmappable("a JSON parameter takes an object");
    }
    if string_type(&to) {
        return map_string(literal, &to);
    }
    match to.as_str() {
        "NUMBER" => match literal {
            Literal::Number(_) => Mapped::Same,
            Literal::String(text) if finite_decimal(text) => Mapped::Rewritten((*text).to_string()),
            _ => Mapped::Unmappable("not a number"),
        },
        "INTEGER" => map_integer(literal, i64::from(i32::MIN), i64::from(i32::MAX), false),
        "LONG" => map_integer(literal, -9_007_199_254_740_992, 9_007_199_254_740_992, true),
        "BOOLEAN" => match literal {
            Literal::Bool(_) => Mapped::Same,
            Literal::String("true") => Mapped::Rewritten("true".to_string()),
            Literal::String("false") => Mapped::Rewritten("false".to_string()),
            _ => Mapped::Unmappable("not a boolean"),
        },
        "DATETIME" => match literal {
            Literal::Number(_) => Mapped::Same,
            Literal::String(text) if jiff::Timestamp::from_str(text).is_ok() => Mapped::Same,
            _ => Mapped::Unmappable("not an ISO 8601 timestamp"),
        },
        _ => Mapped::Unmappable("no literal of this type"),
    }
}

fn string_type(to: &str) -> bool {
    matches!(
        to,
        "STRING"
            | "TEXT"
            | "HTML"
            | "HYPERLINK"
            | "IMAGELINK"
            | "PASSWORD"
            | "GUID"
            | "BASETYPENAME"
            | "USERNAME"
            | "GROUPNAME"
            | "PROJECTNAME"
    ) || to.ends_with("NAME")
}

fn map_string(literal: &Literal<'_>, to: &str) -> Mapped {
    let mapped = match literal {
        Literal::String(text) => {
            if to == "GUID" && !guid(text) {
                return Mapped::Unmappable("not a GUID");
            }
            return Mapped::Same;
        }
        Literal::Number(text) => format!("\"{text}\""),
        Literal::Bool(value) => format!("\"{value}\""),
        Literal::Null => unreachable!("null was handled above"),
    };
    Mapped::Rewritten(mapped)
}

fn map_integer(literal: &Literal<'_>, min: i64, max: i64, long: bool) -> Mapped {
    let text = match literal {
        Literal::Number(text) | Literal::String(text) => text,
        _ => return Mapped::Unmappable("not a whole number"),
    };
    let whole = text.split_once('.').is_none_or(|(_, fraction)| {
        !fraction.is_empty() && fraction.bytes().all(|byte| byte == b'0')
    });
    let integer = text.split_once('.').map_or(*text, |(integer, _)| integer);
    if !decimal(integer) || !whole {
        return Mapped::Unmappable("not a whole number");
    }
    let value = match integer.parse::<i64>() {
        Ok(value) => value,
        Err(_) if long => return Mapped::Unmappable("beyond what a script number holds exactly"),
        Err(_) => return Mapped::Unmappable("not a whole number"),
    };
    if long && !(min..=max).contains(&value) {
        return Mapped::Unmappable("beyond what a script number holds exactly");
    }
    if !(min..=max).contains(&value) {
        return Mapped::Unmappable("not a whole number");
    }
    let rendered = value.to_string();
    match literal {
        Literal::Number(_) if *text == rendered => Mapped::Same,
        _ => Mapped::Rewritten(rendered),
    }
}

fn decimal(text: &str) -> bool {
    let text = text.strip_prefix('-').unwrap_or(text);
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

fn finite_decimal(text: &str) -> bool {
    !text.is_empty() && !text.trim().ne(text) && text.parse::<f64>().is_ok_and(f64::is_finite)
}

fn guid(text: &str) -> bool {
    [8, 13, 18, 23]
        .iter()
        .all(|at| text.as_bytes().get(*at) == Some(&b'-'))
        && text.len() == 36
        && text
            .bytes()
            .enumerate()
            .all(|(at, byte)| matches!(at, 8 | 13 | 18 | 23) || byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::{map_literal, Literal, Mapped};

    fn maps(literal: Literal<'_>, to: &str) -> Mapped {
        map_literal(&literal, to)
    }

    #[test]
    fn maps_scalars_and_string_types() {
        assert_eq!(
            maps(Literal::Number("5"), "STRING"),
            Mapped::Rewritten("\"5\"".into())
        );
        assert_eq!(
            maps(Literal::Bool(true), "THINGNAME"),
            Mapped::Rewritten("\"true\"".into())
        );
        assert_eq!(maps(Literal::String("x"), "TEXT"), Mapped::Same);
        assert_eq!(
            maps(Literal::String("bad"), "GUID"),
            Mapped::Unmappable("not a GUID")
        );
        assert_eq!(
            maps(
                Literal::String("00000000-0000-0000-0000-000000000000"),
                "GUID"
            ),
            Mapped::Same
        );
    }

    #[test]
    fn maps_numbers_and_whole_numbers_at_boundaries() {
        assert_eq!(
            maps(Literal::String("12.5"), "NUMBER"),
            Mapped::Rewritten("12.5".into())
        );
        assert_eq!(
            maps(Literal::String(" 5"), "NUMBER"),
            Mapped::Unmappable("not a number")
        );
        assert_eq!(
            maps(Literal::String("2147483647"), "INTEGER"),
            Mapped::Rewritten("2147483647".into())
        );
        assert_eq!(
            maps(Literal::String("2147483648"), "INTEGER"),
            Mapped::Unmappable("not a whole number")
        );
        assert_eq!(
            maps(Literal::String("9007199254740992"), "LONG"),
            Mapped::Rewritten("9007199254740992".into())
        );
        assert_eq!(
            maps(Literal::String("9007199254740993"), "LONG"),
            Mapped::Unmappable("beyond what a script number holds exactly")
        );
        assert_eq!(
            maps(Literal::Number("5.0"), "LONG"),
            Mapped::Rewritten("5".into())
        );
        assert_eq!(
            maps(Literal::Number("1e3"), "LONG"),
            Mapped::Unmappable("not a whole number")
        );
        assert_eq!(
            maps(Literal::Number("-0"), "INTEGER"),
            Mapped::Rewritten("0".into())
        );
    }

    #[test]
    fn maps_booleans_datetime_and_compound_types() {
        assert_eq!(
            maps(Literal::String("true"), "BOOLEAN"),
            Mapped::Rewritten("true".into())
        );
        assert_eq!(
            maps(Literal::String("TRUE"), "BOOLEAN"),
            Mapped::Unmappable("not a boolean")
        );
        assert_eq!(
            maps(Literal::String("2024-01-01T00:00:00Z"), "DATETIME"),
            Mapped::Same
        );
        assert_eq!(
            maps(Literal::String(""), "DATETIME"),
            Mapped::Unmappable("not an ISO 8601 timestamp")
        );
        assert_eq!(
            maps(Literal::String("{}"), "JSON"),
            Mapped::Unmappable("a JSON parameter takes an object")
        );
        assert_eq!(
            maps(Literal::Number("1"), "LOCATION"),
            Mapped::Unmappable("no literal of this type")
        );
        assert_eq!(maps(Literal::Null, "BOOLEAN"), Mapped::Same);
    }
}
