//! Column-width measurement: the display width of a character or string.

mod tables;

use std::cmp::Ordering;

use tables::{COMBINING, EMOJI, FORMAT, MODIFIER_BASE, WIDE};

/// Joins the emoji before it into the glyph that follows.
const JOINER: char = '\u{200D}';

/// Variation selector asking for the emoji form of the character before it.
const EMOJI_SELECTOR: char = '\u{FE0F}';

/// Skin-tone modifiers, drawn as part of the modifier base before them.
const SKIN_TONES: std::ops::RangeInclusive<char> = '\u{1F3FB}'..='\u{1F3FF}';

/// Regional indicators, which pair into flags and keep one column each.
const REGIONAL_INDICATORS: std::ops::RangeInclusive<char> = '\u{1F1E6}'..='\u{1F1FF}';

/// Display width of a string in columns, laid out from the start of a line.
pub(crate) fn display_width(text: &str) -> usize {
    measure(text, true)
}

/// Display width of a string in columns when it continues a line: a leading combining mark attaches
/// to the character already there and claims no column of its own.
#[cfg_attr(
    not(any(feature = "gfm", feature = "markdown")),
    allow(dead_code, reason = "used by the pipe-table writers")
)]
pub(crate) fn continued_width(text: &str) -> usize {
    measure(text, false)
}

/// Display width of a string in columns when each character is laid out on its own: no cluster
/// folds into a single glyph and every combining mark claims a column.
#[cfg_attr(
    not(feature = "org"),
    allow(dead_code, reason = "used by the org writer")
)]
pub(crate) fn per_character_width(text: &str) -> usize {
    if let Some(width) = ascii_width(text) {
        return width;
    }
    text.chars().fold(0usize, |total, ch| {
        total.saturating_add(width_in_context(ch, "", true))
    })
}

/// Sum the widths of `text`, treating `opening` as the start of a line.
fn measure(text: &str, opening: bool) -> usize {
    if let Some(width) = ascii_width(text) {
        return width;
    }
    let mut total = 0usize;
    let mut rest = text;
    let mut opening = opening;
    while let Some(ch) = rest.chars().next() {
        let tail = rest.get(ch.len_utf8()..).unwrap_or_default();
        total = total.saturating_add(width_in_context(ch, tail, opening));
        opening = false;
        rest = tail;
    }
    total
}

/// The width of a printable-ASCII string, the overwhelming majority of measured text: one column
/// per byte, whichever way the string is laid out. `None` once any other byte appears.
fn ascii_width(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    bytes
        .iter()
        .all(|byte| byte.wrapping_sub(0x20) < 0x5F)
        .then_some(bytes.len())
}

/// Display width of a character: zero for combining marks, formatting code points and controls,
/// two for wide characters, one otherwise.
pub(crate) fn char_width(ch: char) -> usize {
    let code = ch as u32;
    if code < 0x20 || (0x7F..=0x9F).contains(&code) {
        return 0;
    }
    // Nothing below the soft hyphen is zero-width or wide.
    if code < 0x00AD {
        return 1;
    }
    if contains(COMBINING, code) || contains(FORMAT, code) {
        return 0;
    }
    if is_wide(code) { 2 } else { 1 }
}

/// Whether a code point occupies two display columns.
pub(crate) fn is_wide(code: u32) -> bool {
    contains(WIDE, code)
}

/// Columns `ch` claims when `tail` follows it and `opening` marks the start of the text.
fn width_in_context(ch: char, tail: &str, opening: bool) -> usize {
    let mut following = tail.chars();
    let next = following.next();
    if contains(EMOJI, ch as u32) {
        if next == Some(EMOJI_SELECTOR) && takes_emoji_form(ch) {
            // The selector draws the emoji two columns wide, unless a joiner claims it in turn.
            return if following.next() == Some(JOINER) {
                0
            } else {
                2
            };
        }
        if folds_into_next(ch, next) {
            return 0;
        }
    }
    match char_width(ch) {
        0 if opening && contains(COMBINING, ch as u32) => 1,
        width => width,
    }
}

/// Whether the emoji selector widens `ch` from its one-column text form.
fn takes_emoji_form(ch: char) -> bool {
    char_width(ch) == 1 && !REGIONAL_INDICATORS.contains(&ch)
}

/// Whether `next` draws `ch` inside its own glyph, leaving `ch` no columns.
fn folds_into_next(ch: char, next: Option<char>) -> bool {
    match next {
        Some(JOINER) => true,
        Some(following) if SKIN_TONES.contains(&following) => contains(MODIFIER_BASE, ch as u32),
        _ => false,
    }
}

/// Whether a sorted, disjoint table of inclusive ranges covers a code point.
fn contains(table: &[(u32, u32)], code: u32) -> bool {
    table
        .binary_search_by(|&(start, end)| {
            if code < start {
                Ordering::Greater
            } else if code > end {
                Ordering::Less
            } else {
                Ordering::Equal
            }
        })
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_width_classifies_columns() {
        assert_eq!(char_width('a'), 1);
        assert_eq!(char_width('é'), 1);
        assert_eq!(char_width('Ї'), 1);
        assert_eq!(char_width('\n'), 0);
        assert_eq!(char_width('\t'), 0);
        assert_eq!(char_width('\u{7F}'), 0);
        assert_eq!(char_width('\u{85}'), 0);
        assert_eq!(char_width('\u{AD}'), 0);
        assert_eq!(char_width('\u{0301}'), 0);
        assert_eq!(char_width('\u{200B}'), 0);
        assert_eq!(char_width('\u{4E00}'), 2);
        assert_eq!(char_width('\u{FF21}'), 2);
        assert_eq!(char_width('\u{1F600}'), 2);
    }

    #[test]
    fn char_width_follows_the_wide_table_at_its_edges() {
        for code in [
            0x231A, 0x2705, 0x2B1B, 0x2B50, 0x1F004, 0x1F0CF, 0x1FA70, 0x16FE0, 0x17000, 0x18800,
            0x1AFF0, 0x1F680,
        ] {
            assert!(is_wide(code), "{code:#X}");
        }
        for code in [0x3097, 0x3248, 0x3099, 0x1F1E6, 0x1F6E0, 0x2600, 0x2764] {
            assert!(!is_wide(code), "{code:#X}");
        }
    }

    #[test]
    fn display_width_sums_characters() {
        assert_eq!(display_width(""), 0);
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("a\u{4E00}b"), 4);
        assert_eq!(display_width("e\u{0301}"), 1);
    }

    #[test]
    fn opening_combining_marks_claim_one_column() {
        assert_eq!(display_width("\u{0301}"), 1);
        assert_eq!(display_width("\u{0301}\u{0301}\u{0301}"), 1);
        assert_eq!(display_width("\u{0301}a\u{0301}"), 2);
        assert_eq!(display_width("\u{200B}\u{0301}a"), 1);
        assert_eq!(display_width("\u{FE0F}"), 0);
    }

    #[test]
    fn the_emoji_selector_widens_a_text_form() {
        assert_eq!(display_width("\u{2600}"), 1);
        assert_eq!(display_width("\u{2600}\u{FE0F}"), 2);
        assert_eq!(display_width("\u{2600}\u{FE0E}"), 1);
        assert_eq!(display_width("1\u{FE0F}\u{20E3}"), 2);
        assert_eq!(display_width("a\u{FE0F}"), 1);
        assert_eq!(display_width("\u{1F1E6}\u{FE0F}"), 1);
        assert_eq!(display_width("\u{2600}\u{0301}\u{FE0F}"), 1);
    }

    #[test]
    fn continuing_text_leaves_a_leading_mark_no_column() {
        assert_eq!(continued_width("\u{0301}abc"), 3);
        assert_eq!(continued_width("\u{0301}"), 0);
        assert_eq!(continued_width("abc"), 3);
        assert_eq!(continued_width("a\u{0301}bc"), 3);
        assert_eq!(continued_width("\u{2600}\u{FE0F}"), 2);
    }

    #[test]
    fn separate_characters_neither_fold_nor_lose_their_column() {
        assert_eq!(per_character_width("abc"), 3);
        assert_eq!(per_character_width("\u{0301}abc"), 4);
        assert_eq!(per_character_width("a\u{0301}bc"), 4);
        assert_eq!(per_character_width("\u{2600}\u{FE0F}"), 1);
        assert_eq!(per_character_width("1\u{FE0F}\u{20E3}"), 2);
        assert_eq!(
            per_character_width("\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}"),
            6
        );
        assert_eq!(per_character_width("\u{1F469}\u{1F3FB}"), 4);
    }

    #[test]
    fn joined_and_modified_emoji_share_one_glyph() {
        assert_eq!(
            display_width("\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}"),
            2
        );
        assert_eq!(display_width("\u{2764}\u{FE0F}\u{200D}\u{1F525}"), 2);
        assert_eq!(display_width("\u{1F468}\u{FE0F}\u{200D}\u{1F468}"), 4);
        assert_eq!(display_width("\u{1F468}\u{200D}a"), 1);
        assert_eq!(display_width("a\u{200D}\u{1F468}"), 3);
        assert_eq!(display_width("\u{4E00}\u{200D}\u{4E00}"), 4);
        assert_eq!(display_width("\u{1F469}\u{1F3FB}"), 2);
        assert_eq!(display_width("\u{1F6E0}\u{1F3FB}"), 3);
        assert_eq!(display_width("\u{1F1E6}\u{1F1E7}\u{1F1E8}"), 3);
    }
}
