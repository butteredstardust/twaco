//! Whole numbers from JSON NUMBERs, which ThingWorx sends as floats even when they are whole.

/// `value` rounded toward zero as a `u64`. Negative values and NaN read as 0 and values past
/// the range as `u64::MAX`, which is what `as` does from a float; this is the one place that
/// relies on it.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a float-to-integer `as` saturates, and saturating is what is wanted"
)]
pub(crate) fn saturating_u64(value: f64) -> u64 {
    value as u64
}

/// `value` rounded toward zero as an `i64`; NaN reads as 0 and values past the range as the
/// nearest end.
#[allow(
    clippy::cast_possible_truncation,
    reason = "a float-to-integer `as` saturates, and saturating is what is wanted"
)]
pub(crate) fn saturating_i64(value: f64) -> i64 {
    value as i64
}

/// Java's `Double.toString`, which is how ThingWorx writes a NUMBER back: plain decimals with at
/// least one fractional digit from 10^-3 up to 10^7 (`3.0`, `0.0225`), computerized scientific
/// notation outside it (`1.0E7`, `1.0E-4`). The digits are the shortest that read back as the
/// same double, which Rust's formatting also gives.
pub(crate) fn java_double(value: f64) -> String {
    if !value.is_finite() {
        return if value.is_nan() {
            "NaN".into()
        } else if value > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        };
    }
    let magnitude = value.abs();
    if magnitude == 0.0 || (1e-3..1e7).contains(&magnitude) {
        let plain = value.to_string();
        return if plain.contains('.') {
            plain
        } else {
            format!("{plain}.0")
        };
    }
    let scientific = format!("{value:e}");
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    if mantissa.contains('.') {
        format!("{mantissa}E{exponent}")
    } else {
        format!("{mantissa}.0E{exponent}")
    }
}

/// A NUMBER, INTEGER or LONG value given as text, as ThingWorx stores it (`3` -> `3.0`, `+5` ->
/// `5`); `None` when it does not parse as one, so the text is kept as written.
pub(crate) fn java_text(base_type: &str, text: &str) -> Option<String> {
    match base_type {
        "NUMBER" => text
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .map(java_double),
        "INTEGER" | "LONG" => text
            .trim()
            .parse::<i64>()
            .ok()
            .map(|value| value.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a ThingWorx 10.1 server stored for each value sent as a DataShape field's default.
    #[test]
    fn values_read_back_as_the_server_stores_them() {
        for (base_type, sent, stored) in [
            ("NUMBER", "3", "3.0"),
            ("NUMBER", "3.5", "3.5"),
            ("NUMBER", "3.50", "3.5"),
            ("NUMBER", "0", "0.0"),
            ("NUMBER", "-2", "-2.0"),
            ("NUMBER", "-0", "-0.0"),
            ("NUMBER", "+4", "4.0"),
            ("NUMBER", "007", "7.0"),
            ("NUMBER", "1e3", "1000.0"),
            ("NUMBER", "0.1", "0.1"),
            ("NUMBER", "2.25e-2", "0.0225"),
            ("NUMBER", "0.0001", "1.0E-4"),
            ("NUMBER", "0.00001", "1.0E-5"),
            ("NUMBER", "10000000", "1.0E7"),
            ("NUMBER", "12345678", "1.2345678E7"),
            ("INTEGER", "7", "7"),
            ("INTEGER", "07", "7"),
            ("INTEGER", "+5", "5"),
            ("INTEGER", "-3", "-3"),
            ("LONG", "9000000000", "9000000000"),
            ("LONG", "09", "9"),
        ] {
            assert_eq!(
                java_text(base_type, sent).as_deref(),
                Some(stored),
                "{base_type} {sent}"
            );
        }
        assert_eq!(java_text("NUMBER", "lots"), None);
        assert_eq!(java_text("NUMBER", "NaN"), None);
        assert_eq!(java_text("STRING", "3"), None);
        assert_eq!(java_double(1e-3), "0.001");
        assert_eq!(java_double(9_999_999.5), "9999999.5");
    }

    #[test]
    fn floats_saturate_rather_than_wrap() {
        assert_eq!(saturating_u64(42.9), 42);
        assert_eq!(saturating_u64(-3.0), 0);
        assert_eq!(saturating_u64(f64::NAN), 0);
        assert_eq!(saturating_u64(1e300), u64::MAX);
        assert_eq!(saturating_i64(-42.9), -42);
        assert_eq!(saturating_i64(-1e300), i64::MIN);
    }
}
