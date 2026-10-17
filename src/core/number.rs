//! Whole numbers from JSON NUMBERs, which ThingWorx sends as floats even when they are whole.

/// `value` rounded toward zero as a `u64`. Negative values and NaN read as 0 and values past
/// the range as `u64::MAX`, which is what `as` does from a float; this is the one place that
/// relies on it.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a float-to-integer `as` saturates, and saturating is what is wanted"
)]
pub fn saturating_u64(value: f64) -> u64 {
    value as u64
}

/// `value` rounded toward zero as an `i64`; NaN reads as 0 and values past the range as the
/// nearest end.
#[allow(
    clippy::cast_possible_truncation,
    reason = "a float-to-integer `as` saturates, and saturating is what is wanted"
)]
pub fn saturating_i64(value: f64) -> i64 {
    value as i64
}

#[cfg(test)]
mod tests {
    use super::*;

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
