//! Shared numeric formatting for readers that render floating-point values as text.

/// Render a floating-point number in the general decimal form: fixed-point notation when the
/// magnitude lies in `[0.1, 10^7)` and scientific notation otherwise, always carrying at least one
/// fractional digit (`1.0`, never `1`). Zero and non-finite values render as `0.0`.
pub(crate) fn general_decimal(value: f64) -> String {
    if value == 0.0 || !value.is_finite() {
        return "0.0".to_owned();
    }
    let (digits, exponent) = shortest_digits(value.abs());
    let body = if (-1..=6).contains(&exponent) {
        fixed_notation(&digits, exponent)
    } else {
        scientific_notation(&digits, exponent)
    };
    if value.is_sign_negative() {
        format!("-{body}")
    } else {
        body
    }
}

/// The shortest decimal digit run of a positive, finite magnitude together with the power of ten of
/// its leading digit: the value equals `d.ddd… × 10^exponent`. For `0.05` this is (`"5"`, `-2`); for
/// `1234.5`, (`"12345"`, `3`).
fn shortest_digits(magnitude: f64) -> (String, i32) {
    let formatted = format!("{magnitude:e}");
    let (mantissa, exponent) = match formatted.split_once('e') {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i32>().unwrap_or(0)),
        None => (formatted.as_str(), 0),
    };
    let digits = mantissa.chars().filter(char::is_ascii_digit).collect();
    (digits, exponent)
}

/// Lay out a digit run in fixed-point notation given the leading digit's power of ten. Called only
/// for an exponent in `-1..=6`, so a value below one places its single leading digit just after the
/// point.
fn fixed_notation(digits: &str, exponent: i32) -> String {
    if exponent < 0 {
        let leading_zeros = usize::try_from((-exponent - 1).max(0)).unwrap_or(0);
        return format!("0.{}{digits}", "0".repeat(leading_zeros));
    }
    let integer_len = usize::try_from(exponent).unwrap_or(0) + 1;
    if digits.len() <= integer_len {
        let trailing_zeros = integer_len - digits.len();
        format!("{digits}{}.0", "0".repeat(trailing_zeros))
    } else {
        let (integer_part, fraction) = digits.split_at(integer_len);
        format!("{integer_part}.{fraction}")
    }
}

/// Lay out a digit run in scientific notation: one digit before the point, the rest after (`0` when
/// there are none), then the exponent (no `+` sign for a non-negative exponent).
fn scientific_notation(digits: &str, exponent: i32) -> String {
    let (first, rest) = digits.split_at(1.min(digits.len()));
    let mantissa = if rest.is_empty() {
        format!("{first}.0")
    } else {
        format!("{first}.{rest}")
    };
    format!("{mantissa}e{exponent}")
}
