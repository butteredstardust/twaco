//! Durations as a person writes them on the command line: `500ms`, `30s`, `15m`, `2h`, `1d`, a
//! compound such as `1h30m`, or ISO 8601 (`PT15M`). jiff reads them; this is the one grammar every
//! flag that takes a duration shares.

use jiff::{Span, SpanRelativeTo, Unit};

/// `text` as whole milliseconds, more than zero. A day is 24 hours. Weeks, months and years are
/// refused, since a server timer or a log window is never meant in calendar units, and so is a
/// fraction of a millisecond. `None` when `text` is not such a duration.
pub(crate) fn positive_ms(text: &str) -> Option<u64> {
    let span: Span = text.trim().parse().ok()?;
    if span.get_years() != 0 || span.get_months() != 0 || span.get_weeks() != 0 {
        return None;
    }
    let total = span
        .total((Unit::Millisecond, SpanRelativeTo::days_are_24_hours()))
        .ok()?;
    // Exact for every whole number of milliseconds up to 2^53, which is some 285,000 years.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let whole = total as u64;
    (total >= 1.0 && total.fract() == 0.0 && total < 2f64.powi(53)).then_some(whole)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_and_compounds_read_as_milliseconds() {
        for (text, ms) in [
            ("500ms", 500),
            ("30s", 30_000),
            ("1.5s", 1_500),
            ("15m", 900_000),
            ("2h", 7_200_000),
            ("1d", 86_400_000),
            ("1h30m", 5_400_000),
            (" PT15M ", 900_000),
        ] {
            assert_eq!(positive_ms(text), Some(ms), "{text}");
        }
    }

    #[test]
    fn nothing_negative_zero_fractional_or_calendar_is_a_duration() {
        for text in ["", "h", "1", "0m", "-1h", "x5m", "2.5ms", "1w", "1mo", "1y"] {
            assert_eq!(positive_ms(text), None, "{text:?}");
        }
    }
}
