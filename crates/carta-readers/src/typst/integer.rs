//! Signed integers of unbounded width, so code-mode arithmetic keeps every digit.

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

/// The number of decimal digits one limb holds.
const LIMB_DIGITS: usize = 9;

/// The radix each limb counts in, which is ten raised to `LIMB_DIGITS`.
const LIMB_BASE: u64 = 1_000_000_000;

/// The widest result `checked_pow` and `checked_factorial` will build, which keeps a short source
/// expression from asking for gigabytes of digits.
const LIMB_CEILING: usize = 2_500;

/// An integer of unbounded width.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Integer {
    /// Whether the magnitude counts downwards; never set while the magnitude is zero.
    negative: bool,
    /// Base-`LIMB_BASE` digits, least significant first, with no leading zero limb.
    limbs: Vec<u32>,
}

impl Integer {
    /// The additive identity.
    pub(crate) const fn zero() -> Self {
        Self {
            negative: false,
            limbs: Vec::new(),
        }
    }

    /// The multiplicative identity.
    pub(crate) fn one() -> Self {
        Self::from(1i64)
    }

    /// Whether the value is zero.
    pub(crate) fn is_zero(&self) -> bool {
        self.limbs.is_empty()
    }

    /// Whether the value counts downwards from zero.
    pub(crate) fn is_negative(&self) -> bool {
        self.negative
    }

    /// Whether the value divides evenly by two.
    pub(crate) fn is_even(&self) -> bool {
        // Every power of the radix is even, so only the lowest limb decides.
        self.limbs.first().is_none_or(|limb| limb % 2 == 0)
    }

    /// The magnitude, without its sign.
    pub(crate) fn abs(&self) -> Self {
        Self {
            negative: false,
            limbs: self.limbs.clone(),
        }
    }

    /// The value with its sign flipped.
    pub(crate) fn negate(&self) -> Self {
        Self::assemble(!self.negative, self.limbs.clone())
    }

    /// The sum of two values.
    pub(crate) fn add(&self, other: &Self) -> Self {
        if self.negative == other.negative {
            return Self::assemble(self.negative, add_magnitude(&self.limbs, &other.limbs));
        }
        match compare_magnitude(&self.limbs, &other.limbs) {
            Ordering::Less => Self::assemble(
                other.negative,
                subtract_magnitude(&other.limbs, &self.limbs),
            ),
            _ => Self::assemble(self.negative, subtract_magnitude(&self.limbs, &other.limbs)),
        }
    }

    /// The difference of two values.
    pub(crate) fn subtract(&self, other: &Self) -> Self {
        self.add(&other.negate())
    }

    /// The product of two values.
    pub(crate) fn multiply(&self, other: &Self) -> Self {
        Self::assemble(
            self.negative != other.negative,
            multiply_magnitude(&self.limbs, &other.limbs),
        )
    }

    /// The quotient and remainder of division truncated towards zero, or `None` when the divisor
    /// is zero. The remainder takes the dividend's sign.
    pub(crate) fn divide(&self, other: &Self) -> Option<(Self, Self)> {
        if other.is_zero() {
            return None;
        }
        let (quotient, remainder) = divide_magnitude(&self.limbs, &other.limbs);
        Some((
            Self::assemble(self.negative != other.negative, quotient),
            Self::assemble(self.negative, remainder),
        ))
    }

    /// The product of two values, or `None` once the result would pass the digit ceiling.
    pub(crate) fn checked_multiply(&self, other: &Self) -> Option<Self> {
        if self.limbs.len().saturating_add(other.limbs.len()) > LIMB_CEILING {
            return None;
        }
        Some(self.multiply(other))
    }

    /// Whether the value stays inside the digit ceiling that bounds evaluation.
    pub(crate) fn is_bounded(&self) -> bool {
        self.limbs.len() <= LIMB_CEILING
    }

    /// The value raised to a whole power, or `None` once the result would pass the digit ceiling.
    pub(crate) fn checked_pow(&self, exponent: u32) -> Option<Self> {
        let steps = usize::try_from(exponent).unwrap_or(usize::MAX);
        if self.limbs.len().saturating_mul(steps) > LIMB_CEILING {
            return None;
        }
        let mut result = Self::one();
        let mut base = self.clone();
        let mut rest = exponent;
        while rest > 0 {
            if rest % 2 == 1 {
                result = result.multiply(&base);
            }
            rest /= 2;
            if rest > 0 {
                base = base.multiply(&base);
            }
        }
        Some(result)
    }

    /// The product of the whole numbers from one up to `count`, or `None` once it would pass the
    /// digit ceiling.
    pub(crate) fn checked_factorial(count: u32) -> Option<Self> {
        let mut product = Self::one();
        for factor in 2..=count {
            product = product.checked_multiply(&Self::from(i64::from(factor)))?;
        }
        Some(product)
    }

    /// The greatest common divisor of two values, as a magnitude.
    pub(crate) fn greatest_common_divisor(&self, other: &Self) -> Self {
        let mut larger = self.abs();
        let mut smaller = other.abs();
        while !smaller.is_zero() {
            let Some((_, rest)) = larger.divide(&smaller) else {
                break;
            };
            larger = smaller;
            smaller = rest;
        }
        larger
    }

    /// The value as a machine integer, or `None` when it does not fit.
    pub(crate) fn to_i64(&self) -> Option<i64> {
        let mut magnitude: u64 = 0;
        for limb in self.limbs.iter().rev() {
            magnitude = magnitude
                .checked_mul(LIMB_BASE)?
                .checked_add(u64::from(*limb))?;
        }
        if !self.negative {
            return i64::try_from(magnitude).ok();
        }
        match i64::try_from(magnitude) {
            Ok(value) => Some(-value),
            // The negative range reaches one further than the positive one.
            Err(_) if magnitude == 1u64 << 63 => Some(i64::MIN),
            Err(_) => None,
        }
    }

    /// The value as an index, or `None` when it is negative or too wide.
    pub(crate) fn to_usize(&self) -> Option<usize> {
        self.to_i64().and_then(|value| usize::try_from(value).ok())
    }

    /// The value as a float, rounded when the width outruns the mantissa.
    pub(crate) fn to_f64(&self) -> f64 {
        // The literal is `LIMB_BASE`, which a float holds exactly.
        let magnitude = self
            .limbs
            .iter()
            .rev()
            .fold(0.0f64, |total, limb| total.mul_add(1e9, f64::from(*limb)));
        if self.negative { -magnitude } else { magnitude }
    }

    /// The whole part of a float.
    pub(crate) fn from_f64(value: f64) -> Self {
        if !value.is_finite() {
            return Self::zero();
        }
        let truncated = value.trunc();
        #[allow(clippy::cast_possible_truncation)]
        if truncated.abs() < 9.0e18 {
            return Self::from(truncated as i64);
        }
        format!("{truncated:.0}").parse().unwrap_or_default()
    }

    /// Build a value from a sign and unnormalized limbs.
    fn assemble(negative: bool, mut limbs: Vec<u32>) -> Self {
        trim(&mut limbs);
        Self {
            negative: negative && !limbs.is_empty(),
            limbs,
        }
    }
}

impl From<i64> for Integer {
    fn from(value: i64) -> Self {
        let mut magnitude = value.unsigned_abs();
        let mut limbs = Vec::new();
        while magnitude > 0 {
            limbs.push(u32::try_from(magnitude % LIMB_BASE).unwrap_or_default());
            magnitude /= LIMB_BASE;
        }
        Self::assemble(value < 0, limbs)
    }
}

impl From<usize> for Integer {
    fn from(value: usize) -> Self {
        Self::from(i64::try_from(value).unwrap_or(i64::MAX))
    }
}

impl From<i32> for Integer {
    fn from(value: i32) -> Self {
        Self::from(i64::from(value))
    }
}

impl FromStr for Integer {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (negative, digits) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text.strip_prefix('+').unwrap_or(text)),
        };
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(());
        }
        let mut limbs = Vec::new();
        let mut end = digits.len();
        while end > 0 {
            let start = end.saturating_sub(LIMB_DIGITS);
            let chunk = digits.get(start..end).ok_or(())?;
            limbs.push(chunk.parse::<u32>().map_err(|_| ())?);
            end = start;
        }
        Ok(Self::assemble(negative, limbs))
    }
}

impl fmt::Display for Integer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Some(top) = self.limbs.last() else {
            return formatter.write_str("0");
        };
        if self.negative {
            formatter.write_str("-")?;
        }
        write!(formatter, "{top}")?;
        for limb in self.limbs.iter().rev().skip(1) {
            write!(formatter, "{limb:0>LIMB_DIGITS$}")?;
        }
        Ok(())
    }
}

impl Ord for Integer {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.negative, other.negative) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (false, false) => compare_magnitude(&self.limbs, &other.limbs),
            (true, true) => compare_magnitude(&other.limbs, &self.limbs),
        }
    }
}

impl PartialOrd for Integer {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Drop the leading zero limbs that arithmetic leaves behind.
fn trim(limbs: &mut Vec<u32>) {
    while limbs.last() == Some(&0) {
        limbs.pop();
    }
}

/// Order two magnitudes, longest first and then by their leading digits.
fn compare_magnitude(left: &[u32], right: &[u32]) -> Ordering {
    left.len()
        .cmp(&right.len())
        .then_with(|| left.iter().rev().cmp(right.iter().rev()))
}

/// The sum of two magnitudes.
fn add_magnitude(left: &[u32], right: &[u32]) -> Vec<u32> {
    let width = left.len().max(right.len());
    let mut out = Vec::with_capacity(width.saturating_add(1));
    let mut carry = 0u64;
    for index in 0..width {
        let total = carry
            .saturating_add(u64::from(left.get(index).copied().unwrap_or_default()))
            .saturating_add(u64::from(right.get(index).copied().unwrap_or_default()));
        out.push(u32::try_from(total % LIMB_BASE).unwrap_or_default());
        carry = total / LIMB_BASE;
    }
    if carry > 0 {
        out.push(u32::try_from(carry).unwrap_or_default());
    }
    trim(&mut out);
    out
}

/// The difference of two magnitudes, where the left one is the larger.
fn subtract_magnitude(left: &[u32], right: &[u32]) -> Vec<u32> {
    let mut out = Vec::with_capacity(left.len());
    let mut borrow = 0u64;
    for index in 0..left.len() {
        let minuend = u64::from(left.get(index).copied().unwrap_or_default());
        let subtrahend =
            u64::from(right.get(index).copied().unwrap_or_default()).saturating_add(borrow);
        let (digit, next) = if minuend >= subtrahend {
            (minuend.saturating_sub(subtrahend), 0)
        } else {
            (
                minuend.saturating_add(LIMB_BASE).saturating_sub(subtrahend),
                1,
            )
        };
        out.push(u32::try_from(digit).unwrap_or_default());
        borrow = next;
    }
    trim(&mut out);
    out
}

/// The product of two magnitudes.
fn multiply_magnitude(left: &[u32], right: &[u32]) -> Vec<u32> {
    if left.is_empty() || right.is_empty() {
        return Vec::new();
    }
    let mut wide = vec![0u64; left.len().saturating_add(right.len())];
    for (offset, factor) in left.iter().enumerate() {
        let mut carry = 0u64;
        for (index, other) in right.iter().enumerate() {
            let Some(slot) = wide.get_mut(offset.saturating_add(index)) else {
                continue;
            };
            let total = *slot + u64::from(*factor) * u64::from(*other) + carry;
            *slot = total % LIMB_BASE;
            carry = total / LIMB_BASE;
        }
        if let Some(slot) = wide.get_mut(offset.saturating_add(right.len())) {
            *slot = slot.saturating_add(carry);
        }
    }
    let mut out = Vec::with_capacity(wide.len());
    let mut carry = 0u64;
    for slot in wide {
        let total = slot.saturating_add(carry);
        out.push(u32::try_from(total % LIMB_BASE).unwrap_or_default());
        carry = total / LIMB_BASE;
    }
    while carry > 0 {
        out.push(u32::try_from(carry % LIMB_BASE).unwrap_or_default());
        carry /= LIMB_BASE;
    }
    trim(&mut out);
    out
}

/// The quotient and remainder of two magnitudes, by long division that searches each digit.
fn divide_magnitude(dividend: &[u32], divisor: &[u32]) -> (Vec<u32>, Vec<u32>) {
    let mut quotient = vec![0u32; dividend.len()];
    let mut remainder: Vec<u32> = Vec::new();
    let highest = u32::try_from(LIMB_BASE.saturating_sub(1)).unwrap_or(u32::MAX);
    for index in (0..dividend.len()).rev() {
        remainder.insert(0, dividend.get(index).copied().unwrap_or_default());
        trim(&mut remainder);
        let (mut low, mut high) = (0u32, highest);
        while low < high {
            let middle = low.saturating_add(high.saturating_sub(low).saturating_add(1) / 2);
            if compare_magnitude(&multiply_magnitude(divisor, &[middle]), &remainder)
                == Ordering::Greater
            {
                high = middle.saturating_sub(1);
            } else {
                low = middle;
            }
        }
        if low > 0 {
            remainder = subtract_magnitude(&remainder, &multiply_magnitude(divisor, &[low]));
        }
        if let Some(slot) = quotient.get_mut(index) {
            *slot = low;
        }
    }
    trim(&mut quotient);
    (quotient, remainder)
}

#[cfg(test)]
mod tests {
    use super::Integer;

    fn parse(text: &str) -> Integer {
        text.parse().unwrap_or_else(|()| Integer::zero())
    }

    #[test]
    fn parses_and_prints_wide_values() {
        let text = "123456789012345678901234567890";
        assert_eq!(parse(text).to_string(), text);
        assert_eq!(parse("-42").to_string(), "-42");
        assert_eq!(parse("000").to_string(), "0");
        assert_eq!(Integer::zero().to_string(), "0");
        assert!("12a".parse::<Integer>().is_err());
        assert!("".parse::<Integer>().is_err());
    }

    #[test]
    fn adds_and_subtracts_across_the_sign() {
        assert_eq!(
            parse("9223372036854775807")
                .add(&Integer::one())
                .to_string(),
            "9223372036854775808"
        );
        assert_eq!(
            parse("-9223372036854775808")
                .subtract(&Integer::one())
                .to_string(),
            "-9223372036854775809"
        );
        assert_eq!(parse("5").subtract(&parse("12")).to_string(), "-7");
        assert!(parse("7").subtract(&parse("7")).is_zero());
    }

    #[test]
    fn multiplies_and_divides_exactly() {
        assert_eq!(
            parse("99999999999")
                .multiply(&parse("99999999999"))
                .to_string(),
            "9999999999800000000001"
        );
        let (quotient, remainder) = parse("100000000000000000000")
            .divide(&parse("3"))
            .unwrap_or((Integer::zero(), Integer::zero()));
        assert_eq!(quotient.to_string(), "33333333333333333333");
        assert_eq!(remainder.to_string(), "1");
        assert_eq!(
            parse("-7")
                .divide(&parse("2"))
                .map(|(q, r)| (q.to_string(), r.to_string())),
            Some(("-3".to_string(), "-1".to_string()))
        );
        assert_eq!(parse("1").divide(&Integer::zero()), None);
    }

    #[test]
    fn raises_powers_and_factorials() {
        assert_eq!(
            parse("2").checked_pow(100).map(|value| value.to_string()),
            Some("1267650600228229401496703205376".to_string())
        );
        assert_eq!(
            Integer::checked_factorial(21).map(|value| value.to_string()),
            Some("51090942171709440000".to_string())
        );
        assert_eq!(parse("10").checked_pow(u32::MAX), None);
        assert_eq!(Integer::checked_factorial(u32::MAX), None);
    }

    #[test]
    fn converts_to_machine_types() {
        assert_eq!(parse("-9223372036854775808").to_i64(), Some(i64::MIN));
        assert_eq!(parse("9223372036854775808").to_i64(), None);
        assert_eq!(parse("-1").to_usize(), None);
        assert!((parse("1267650600228229401496703205376").to_f64() - 2f64.powi(100)).abs() < 1.0);
        assert_eq!(
            Integer::from_f64(1e30).to_string(),
            "1000000000000000019884624838656"
        );
        assert!(Integer::from_f64(f64::NAN).is_zero());
    }

    #[test]
    fn orders_and_divides_common_factors() {
        assert!(parse("-5") < parse("3"));
        assert!(parse("100000000000000000000") > parse("99999999999999999999"));
        assert_eq!(
            parse("48")
                .greatest_common_divisor(&parse("-18"))
                .to_string(),
            "6"
        );
        assert!(parse("10").is_even());
        assert!(!parse("1000000001").is_even());
    }
}
