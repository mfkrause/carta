//! Inline scanners and folds shared by the `CommonMark` and HTML readers.
//!
//! These are stateless scanning utilities: each takes the source as a `&[char]` or `&str` and a
//! cursor position (or a length), so both readers can drive them from their own inline machinery
//! without coupling to either parser's state. The `CommonMark` reader works over byte offsets into a
//! `&str` (the `_bytes` variants) while the HTML reader works over a `&[char]`. The TeX-math scanners
//! recognize the same delimiter shapes the readers gate behind their math extensions; the
//! dash fold backs the `smart` typography.

use carta_ast::MathType;

/// Scan inline math `$…$` starting at the opening `$` (at `pos`). Returns the content and the index
/// past the closing `$`, or `None` if no valid `$…$` begins here.
///
/// Inline math opens only when the `$` is followed by a non-space character, and closes at the next
/// unescaped `$` that is preceded by a non-space and not followed by a digit. A backslash escapes
/// the next character, so the content never holds an unescaped `$`.
#[cfg(feature = "html")]
pub(crate) fn scan_inline_math(chars: &[char], pos: usize) -> Option<(String, usize)> {
    if chars
        .get(pos + 1)
        .copied()
        .is_none_or(is_unicode_whitespace)
    {
        return None;
    }
    let content_start = pos + 1;
    let mut i = content_start;
    while let Some(&ch) = chars.get(i) {
        if ch == '\\' && chars.get(i + 1).is_some() {
            i += 2;
            continue;
        }
        if ch == '$' {
            let prev_space = chars.get(i - 1).copied().is_none_or(is_unicode_whitespace);
            let next_digit = chars.get(i + 1).is_some_and(char::is_ascii_digit);
            if prev_space || next_digit {
                return None;
            }
            let content: String = chars.get(content_start..i)?.iter().collect();
            return Some((content, i + 1));
        }
        i += 1;
    }
    None
}

/// Scan display math `$$…$$` starting at the opening `$$` (at `pos`). Returns the content and the
/// index past the closing `$$`, or `None` if no closing `$$` follows.
#[cfg(feature = "html")]
pub(crate) fn scan_display_math(chars: &[char], pos: usize) -> Option<(String, usize)> {
    let content_start = pos + 2;
    let mut i = content_start;
    while chars.get(i).is_some() {
        if chars.get(i) == Some(&'$') && chars.get(i + 1) == Some(&'$') {
            let content: String = chars.get(content_start..i)?.iter().collect();
            return Some((content, i + 2));
        }
        i += 1;
    }
    None
}

/// Scan a backslash math span whose delimiters are `slashes` backslashes followed by `(` (inline)
/// or `[` (display). `pos` is on the first backslash; `slashes` counts the backslashes of the
/// opener. The content runs to the matching `slashes`-backslash + `)`/`]` closer, honoring
/// `\`-escapes inside the single-backslash form, and must be non-empty before any trimming. Inline
/// content is trimmed of surrounding whitespace; display content is kept verbatim. Returns the math
/// type, its content, and the index past the closer, or `None` on no match.
#[cfg(feature = "html")]
pub(crate) fn scan_backslash_math(
    chars: &[char],
    pos: usize,
    slashes: usize,
) -> Option<(MathType, String, usize)> {
    let open = pos + slashes;
    let (close_bracket, math_type) = match chars.get(open).copied() {
        Some('(') => (')', MathType::InlineMath),
        Some('[') => (']', MathType::DisplayMath),
        _ => return None,
    };
    let content_start = open + 1;
    let mut i = content_start;
    while chars.get(i).is_some() {
        if is_backslash_math_closer(chars, i, slashes, close_bracket) {
            if i == content_start {
                return None; // empty content is not a math span
            }
            let raw: String = chars.get(content_start..i)?.iter().collect();
            let content = match math_type {
                MathType::InlineMath => raw.trim().to_owned(),
                MathType::DisplayMath => raw,
            };
            return Some((math_type, content, i + slashes + 1));
        }
        // The single-backslash form treats a `\` as escaping the next character, so an escaped
        // delimiter inside the content does not close the span; the closer test above already ran,
        // so a real `\)`/`\]` closer is never reached here. The double-backslash form has no such
        // escaping: a longer backslash run simply leaves its leading backslashes in the content.
        if slashes == 1 && chars.get(i) == Some(&'\\') && chars.get(i + 1).is_some() {
            i += 2;
            continue;
        }
        i += 1;
    }
    None
}

/// Whether a `slashes`-backslash run followed by `close` begins at index `i`.
#[cfg(feature = "html")]
fn is_backslash_math_closer(chars: &[char], i: usize, slashes: usize, close: char) -> bool {
    (0..slashes).all(|k| chars.get(i + k) == Some(&'\\')) && chars.get(i + slashes) == Some(&close)
}

#[cfg(feature = "commonmark")]
use crate::commonmark::scan::char_at;

/// The character ending just before byte offset `at`, or `None` at the start of `text`.
#[cfg(feature = "commonmark")]
fn char_before(text: &str, at: usize) -> Option<char> {
    text.get(..at).and_then(|head| head.chars().next_back())
}

/// Byte-offset twin of [`scan_inline_math`]: `pos` is the byte offset of the opening `$`, and the
/// returned end position is a byte offset.
#[cfg(feature = "commonmark")]
pub(crate) fn scan_inline_math_bytes(text: &str, pos: usize) -> Option<(String, usize)> {
    if char_at(text, pos + 1).is_none_or(is_unicode_whitespace) {
        return None;
    }
    let content_start = pos + 1;
    let mut i = content_start;
    while let Some(ch) = char_at(text, i) {
        if ch == '\\'
            && let Some(next) = char_at(text, i + 1)
        {
            i += 1 + next.len_utf8();
            continue;
        }
        if ch == '$' {
            let prev_space = char_before(text, i).is_none_or(is_unicode_whitespace);
            let next_digit = char_at(text, i + 1).is_some_and(|c| c.is_ascii_digit());
            if prev_space || next_digit {
                return None;
            }
            let content = text.get(content_start..i)?.to_owned();
            return Some((content, i + 1));
        }
        i += ch.len_utf8();
    }
    None
}

/// Byte-offset twin of [`scan_display_math`]: `pos` is the byte offset of the opening `$$`, and the
/// returned end position is a byte offset.
#[cfg(feature = "commonmark")]
pub(crate) fn scan_display_math_bytes(text: &str, pos: usize) -> Option<(String, usize)> {
    let content_start = pos + 2;
    let mut i = content_start;
    while let Some(ch) = char_at(text, i) {
        if text.get(i..).is_some_and(|rest| rest.starts_with("$$")) {
            let content = text.get(content_start..i)?.to_owned();
            return Some((content, i + 2));
        }
        i += ch.len_utf8();
    }
    None
}

/// Byte-offset twin of [`scan_backslash_math`]: `pos` is the byte offset of the first backslash, and
/// the returned end position is a byte offset.
#[cfg(feature = "commonmark")]
pub(crate) fn scan_backslash_math_bytes(
    text: &str,
    pos: usize,
    slashes: usize,
) -> Option<(MathType, String, usize)> {
    let open = pos + slashes;
    let (close_bracket, math_type) = match char_at(text, open) {
        Some('(') => (')', MathType::InlineMath),
        Some('[') => (']', MathType::DisplayMath),
        _ => return None,
    };
    let content_start = open + 1;
    let mut i = content_start;
    while let Some(ch) = char_at(text, i) {
        if is_backslash_math_closer_bytes(text, i, slashes, close_bracket) {
            if i == content_start {
                return None; // empty content is not a math span
            }
            let raw = text.get(content_start..i)?.to_owned();
            let content = match math_type {
                MathType::InlineMath => raw.trim().to_owned(),
                MathType::DisplayMath => raw,
            };
            return Some((math_type, content, i + slashes + 1));
        }
        if slashes == 1
            && ch == '\\'
            && let Some(next) = char_at(text, i + 1)
        {
            i += 1 + next.len_utf8();
            continue;
        }
        i += ch.len_utf8();
    }
    None
}

/// Byte-offset twin of [`is_backslash_math_closer`].
#[cfg(feature = "commonmark")]
fn is_backslash_math_closer_bytes(text: &str, i: usize, slashes: usize, close: char) -> bool {
    (0..slashes).all(|k| text.as_bytes().get(i + k) == Some(&b'\\'))
        && char_at(text, i + slashes) == Some(close)
}

/// A forward-scan step budget proportional to `span_len`. A dense run of unclosable
/// openers would otherwise make each failed construct re-scan the whole suffix, so
/// the total cost grows quadratically; charging one step per position examined keeps
/// scanning linear over the span. The value is far above what any genuine construct
/// needs — a real close is always found — while the cap makes a pathological run give
/// up and leave the opener as literal text.
#[cfg(any(feature = "jira", feature = "mediawiki", feature = "org"))]
pub(crate) fn scan_budget(span_len: usize) -> usize {
    span_len.saturating_mul(8).saturating_add(64).min(200_000)
}

/// Whether `ch` is whitespace for the inline scanners: the spec's literal whitespace set plus any
/// Unicode whitespace.
pub(crate) fn is_unicode_whitespace(ch: char) -> bool {
    ch == ' '
        || ch == '\t'
        || ch == '\n'
        || ch == '\u{0c}'
        || ch == '\u{0b}'
        || ch == '\r'
        || ch.is_whitespace()
}

#[cfg(all(test, any(feature = "jira", feature = "mediawiki", feature = "org")))]
mod scan_budget_tests {
    use super::scan_budget;

    #[test]
    fn scan_budget_scales_then_caps() {
        assert_eq!(scan_budget(0), 64);
        assert_eq!(scan_budget(1_000), 8_064);
        assert_eq!(scan_budget(1_000_000), 200_000);
    }
}

#[cfg(all(test, feature = "commonmark"))]
mod tests {
    use super::{scan_display_math_bytes, scan_inline_math_bytes};

    #[test]
    fn inline_math_spans_multi_byte_content() {
        // The content and the returned end position are byte offsets; the closing `$` follows a
        // multi-byte character.
        assert_eq!(
            scan_inline_math_bytes("$αβ$", 0),
            Some(("αβ".to_owned(), 6))
        );
    }

    #[test]
    fn inline_math_opens_after_a_multi_byte_neighbor() {
        // The opener sits at a byte offset past a multi-byte character.
        assert_eq!(scan_inline_math_bytes("é$β$", 2), Some(("β".to_owned(), 6)));
    }

    #[test]
    fn inline_math_escape_hops_over_a_multi_byte_character() {
        // A backslash escapes the following character whole, so an escaped multi-byte character
        // never leaves the cursor mid-character.
        assert_eq!(
            scan_inline_math_bytes("$\\éx$", 0),
            Some(("\\éx".to_owned(), 6))
        );
    }

    #[test]
    fn inline_math_rejects_ill_flanked_closers() {
        // Whitespace before the closing `$`, a digit after it, or whitespace after the opener each
        // void the span.
        assert_eq!(scan_inline_math_bytes("$x $", 0), None);
        assert_eq!(scan_inline_math_bytes("$x$5", 0), None);
        assert_eq!(scan_inline_math_bytes("$ x$", 0), None);
    }

    #[test]
    fn display_math_spans_multi_byte_content() {
        assert_eq!(
            scan_display_math_bytes("$$α+β$$", 0),
            Some(("α+β".to_owned(), 9))
        );
    }

    #[test]
    fn display_math_without_a_closer_is_not_math() {
        // A lone `$` after multi-byte content does not close a `$$` opener.
        assert_eq!(scan_display_math_bytes("$$αβ$", 0), None);
    }
}
