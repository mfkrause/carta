//! List-tightness, block spacing, and ordered-list numerals and delimiters. Consumed by whichever
//! text writers are enabled, so unused-item warnings are allowed here rather than gated per item.
#![allow(dead_code)]

use carta_ast::{Block, ListNumberDelim, ListNumberStyle};

/// Whether a list is tight: every item is empty or opens with a [`Block::Plain`].
pub(crate) fn list_is_tight(items: &[Vec<Block>]) -> bool {
    items
        .iter()
        .all(|item| matches!(item.first(), None | Some(Block::Plain(_))))
}

/// Whether a list is loose — at least one item carries a top-level paragraph. A loose list's items
/// are separated with a blank line and each item's blocks are laid out with blank lines; a tight
/// list uses single newlines throughout.
pub(crate) fn is_loose(items: &[Vec<Block>]) -> bool {
    !list_is_tight(items)
}

/// The separator between two list items at the given layout density: a blank line when loose, a
/// single newline when tight.
pub(crate) fn item_separator(loose: bool) -> &'static str {
    if loose { "\n\n" } else { "\n" }
}

/// Join already-rendered blocks with the document's default blank-line spacing, dropping blocks that
/// produced no output. A [`Block::Plain`] contributes only a single newline (not a blank line)
/// before the next visible block when an empty block falls between them.
pub(crate) fn join_loose(rendered: Vec<(bool, String)>) -> String {
    let mut out = String::new();
    let mut previous_was_plain: Option<bool> = None;
    let mut empty_since_previous = false;
    for (is_plain, text) in rendered {
        if text.is_empty() {
            if previous_was_plain.is_some() {
                empty_since_previous = true;
            }
            continue;
        }
        if let Some(was_plain) = previous_was_plain {
            if was_plain && empty_since_previous {
                out.push('\n');
            } else {
                out.push_str("\n\n");
            }
        }
        out.push_str(&text);
        previous_was_plain = Some(is_plain);
        empty_since_previous = false;
    }
    out
}

/// Wrap an ordered-list numeral in its delimiter: `n.`, `n)`, or `(n)`.
pub(crate) fn wrap_delim(numeral: &str, delim: ListNumberDelim) -> String {
    match delim {
        ListNumberDelim::DefaultDelim | ListNumberDelim::Period => format!("{numeral}."),
        ListNumberDelim::OneParen => format!("{numeral})"),
        ListNumberDelim::TwoParens => format!("({numeral})"),
    }
}

/// The leading marker for an ordered-list item: its number in the list's numeral style, wrapped in
/// the list's delimiter.
pub(crate) fn ordered_marker(
    number: i32,
    style: ListNumberStyle,
    delim: ListNumberDelim,
) -> String {
    wrap_delim(&numeral(number, style), delim)
}

/// Render a number in a list's numeral style.
pub(crate) fn numeral(number: i32, style: ListNumberStyle) -> String {
    match style {
        ListNumberStyle::DefaultStyle | ListNumberStyle::Decimal | ListNumberStyle::Example => {
            number.to_string()
        }
        ListNumberStyle::LowerAlpha => alpha(number, false),
        ListNumberStyle::UpperAlpha => alpha(number, true),
        ListNumberStyle::LowerRoman => roman(number, false),
        ListNumberStyle::UpperRoman => roman(number, true),
    }
}

/// Bijective base-26 alphabetic numeral (1 -> a, 26 -> z, 27 -> aa). Non-positive input falls back
/// to the decimal form, which cannot be expressed as a letter.
pub(crate) fn alpha(number: i32, upper: bool) -> String {
    if number < 1 {
        return number.to_string();
    }
    let base = if upper { b'A' } else { b'a' };
    let mut value = number;
    let mut letters = Vec::new();
    while value > 0 {
        let remainder = (value - 1) % 26;
        letters.push(base + u8::try_from(remainder).unwrap_or(0));
        value = (value - 1) / 26;
    }
    letters.reverse();
    String::from_utf8(letters).unwrap_or_else(|_| number.to_string())
}

/// Roman numeral for a positive number; non-positive input falls back to the decimal form.
pub(crate) fn roman(number: i32, upper: bool) -> String {
    const UNITS: [(i32, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    if number < 1 {
        return number.to_string();
    }
    let mut remaining = number;
    let mut out = String::new();
    for (value, symbol) in UNITS {
        while remaining >= value {
            out.push_str(symbol);
            remaining -= value;
        }
    }
    if upper { out.to_uppercase() } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeral_renders_every_style() {
        assert_eq!(numeral(5, ListNumberStyle::Decimal), "5");
        assert_eq!(numeral(5, ListNumberStyle::DefaultStyle), "5");
        assert_eq!(numeral(5, ListNumberStyle::Example), "5");
        assert_eq!(numeral(1, ListNumberStyle::LowerAlpha), "a");
        assert_eq!(numeral(27, ListNumberStyle::LowerAlpha), "aa");
        assert_eq!(numeral(1, ListNumberStyle::UpperAlpha), "A");
        assert_eq!(numeral(28, ListNumberStyle::UpperAlpha), "AB");
        assert_eq!(numeral(4, ListNumberStyle::LowerRoman), "iv");
        assert_eq!(numeral(9, ListNumberStyle::LowerRoman), "ix");
        assert_eq!(numeral(2024, ListNumberStyle::UpperRoman), "MMXXIV");
    }

    #[test]
    fn numeral_non_positive_falls_back_to_decimal() {
        assert_eq!(alpha(0, false), "0");
        assert_eq!(alpha(-3, true), "-3");
        assert_eq!(roman(0, false), "0");
        assert_eq!(roman(-1, true), "-1");
    }

    #[test]
    fn wrap_delim_and_marker() {
        assert_eq!(wrap_delim("3", ListNumberDelim::Period), "3.");
        assert_eq!(wrap_delim("3", ListNumberDelim::DefaultDelim), "3.");
        assert_eq!(wrap_delim("3", ListNumberDelim::OneParen), "3)");
        assert_eq!(wrap_delim("3", ListNumberDelim::TwoParens), "(3)");
        assert_eq!(
            ordered_marker(2, ListNumberStyle::LowerRoman, ListNumberDelim::OneParen),
            "ii)"
        );
    }

    #[test]
    fn tightness_and_separators() {
        let tight = vec![vec![Block::Plain(vec![])], vec![]];
        let loose = vec![vec![Block::Para(vec![])]];
        assert!(list_is_tight(&tight));
        assert!(!is_loose(&tight));
        assert!(is_loose(&loose));
        assert_eq!(item_separator(true), "\n\n");
        assert_eq!(item_separator(false), "\n");
    }

    #[test]
    fn join_loose_spaces_blocks() {
        let rendered = vec![
            (false, "A".to_owned()),
            (false, String::new()),
            (false, "B".to_owned()),
        ];
        assert_eq!(join_loose(rendered), "A\n\nB");
        let plain_then_empty = vec![
            (true, "x".to_owned()),
            (false, String::new()),
            (true, "y".to_owned()),
        ];
        assert_eq!(join_loose(plain_then_empty), "x\ny");
    }
}
