//! Roman numeral evaluation.
//!
//! Three deliberately distinct algorithms live side by side, one per reader. They disagree on
//! ill-formed numerals — `iix` is 8 to the reverse scan, 10 to the forward lookahead, and rejected
//! by the strict place-wise parser — and each reader's list parsing depends on its own reading, so
//! they are not interchangeable.

/// The value of a roman numeral read right to left: a digit smaller than the largest digit seen so
/// far subtracts, any other digit adds. Accepts ill-formed numerals (`iix` → 8).
#[cfg(feature = "rst")]
pub(crate) fn roman_value_loose_reverse(text: &str) -> Option<i32> {
    let mut total = 0;
    let mut prev = 0;
    for ch in text.chars().rev() {
        let value = match ch.to_ascii_lowercase() {
            'i' => 1,
            'v' => 5,
            'x' => 10,
            'l' => 50,
            'c' => 100,
            'd' => 500,
            'm' => 1000,
            _ => return None,
        };
        if value < prev {
            total -= value;
        } else {
            total += value;
            prev = value;
        }
    }
    if total > 0 { Some(total) } else { None }
}

/// The value of a roman numeral read left to right: a digit smaller than its successor subtracts,
/// any other digit adds. Accepts ill-formed numerals (`iix` → 10). `None` if any character is not a
/// roman digit.
#[cfg(feature = "man")]
pub(crate) fn roman_value_loose_forward(text: &str) -> Option<i32> {
    fn digit(c: char) -> Option<i32> {
        match c.to_ascii_lowercase() {
            'i' => Some(1),
            'v' => Some(5),
            'x' => Some(10),
            'l' => Some(50),
            'c' => Some(100),
            'd' => Some(500),
            'm' => Some(1000),
            _ => None,
        }
    }
    let values: Vec<i32> = text.chars().map(digit).collect::<Option<Vec<_>>>()?;
    let mut total = 0;
    for (index, &value) in values.iter().enumerate() {
        match values.get(index + 1) {
            Some(&next) if value < next => total -= value,
            _ => total += value,
        }
    }
    (total > 0).then_some(total)
}

/// Value of a roman numeral in well-formed place order, or `None` if the run is not a valid numeral.
///
/// The numeral is read place by place — thousands, hundreds, tens, ones — and the whole run must be
/// consumed. Thousands repeat without bound; each lower place takes its subtractive pair (`CM`/`CD`,
/// `XC`/`XL`, `IX`/`IV`), an optional half-digit (`D`/`L`/`V`), and up to four repeats of its unit
/// digit. Ill-formed runs — a repeated half-digit (`VV`), an out-of-order digit (`IIX`), or an
/// invalid subtraction (`IL`) — are rejected.
#[cfg(feature = "commonmark")]
pub(crate) fn roman_value_strict(run: &[u8]) -> Option<i32> {
    let lower: Vec<u8> = run.iter().map(u8::to_ascii_lowercase).collect();
    let mut pos = 0usize;
    let mut total: i32 = 0;

    // Thousands: any number of `m`.
    while lower.get(pos) == Some(&b'm') {
        total = total.checked_add(1000)?;
        pos += 1;
    }
    total += take_roman_place(&lower, &mut pos, b'c', b'd', b'm', 100);
    total += take_roman_place(&lower, &mut pos, b'x', b'l', b'c', 10);
    total += take_roman_place(&lower, &mut pos, b'i', b'v', b'x', 1);

    if pos != lower.len() || total == 0 {
        return None;
    }
    Some(total)
}

/// Read one place of a roman numeral. `unit` is the place's digit (value `unit_value`), `half` is the
/// digit worth five units, and `next` is the digit worth ten units (used by the subtractive forms).
/// Consumes the subtractive pair (`unit`+`next` → nine, `unit`+`half` → four), then an optional half
/// digit, then up to four unit digits, and returns the place's value. A digit that does not belong to
/// this place is left for the next place; an ill-formed run is rejected by [`roman_value_strict`],
/// which requires every byte to be consumed.
#[cfg(feature = "commonmark")]
fn take_roman_place(
    digits: &[u8],
    pos: &mut usize,
    unit: u8,
    half: u8,
    next: u8,
    unit_value: i32,
) -> i32 {
    if digits.get(*pos) == Some(&unit) {
        if digits.get(*pos + 1) == Some(&next) {
            *pos += 2;
            return unit_value * 9;
        }
        if digits.get(*pos + 1) == Some(&half) {
            *pos += 2;
            return unit_value * 4;
        }
    }
    let mut value = 0;
    if digits.get(*pos) == Some(&half) {
        value += unit_value * 5;
        *pos += 1;
    }
    let mut repeats = 0;
    while digits.get(*pos) == Some(&unit) && repeats < 4 {
        value += unit_value;
        *pos += 1;
        repeats += 1;
    }
    value
}

#[cfg(all(test, feature = "commonmark"))]
mod tests {
    use super::roman_value_strict;

    #[test]
    fn roman_value_reads_thousands() {
        assert_eq!(roman_value_strict(b"mm"), Some(2000));
    }

    // A roman run long enough to overflow the thousands accumulator is not a valid enumerator: the
    // checked add yields `None` rather than panicking under overflow checks.
    #[test]
    fn roman_value_rejects_oversized_run() {
        let run = vec![b'm'; 3_000_000];
        assert_eq!(roman_value_strict(&run), None);
    }
}
