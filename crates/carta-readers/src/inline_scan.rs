//! Inline scanners and folds shared by the `CommonMark` and HTML readers.
//!
//! These are stateless scanning utilities: each takes the source as a `&[char]` or `&str` and a
//! cursor position (or a length), so both readers can drive them from their own inline machinery
//! without coupling to either parser's state. The `CommonMark` reader works over byte offsets into a
//! `&str` (the `_bytes` variants) while the HTML reader works over a `&[char]`. The TeX-math scanners
//! recognize the same delimiter shapes the readers gate behind their math extensions; the
//! dash/ellipsis folds back the `smart` typography.

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

/// The character beginning at byte offset `at`, or `None` at or past the end of `text`.
#[cfg(feature = "commonmark")]
fn char_at(text: &str, at: usize) -> Option<char> {
    text.get(at..).and_then(|rest| rest.chars().next())
}

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

/// Fold a run of `len` hyphens (`len >= 2`) into the fewest em (`—`) and en (`–`) dashes that sum to
/// its length: a multiple of three is all em dashes, an even length is all en dashes, and an odd
/// length that is not a multiple of three takes one or two en dashes — whichever leaves a multiple of
/// three — with the rest em dashes.
///
/// The HTML reader folds dash runs greedily instead, so this minimal decomposition is `CommonMark`-only.
#[cfg(feature = "commonmark")]
pub(crate) fn fold_dash_run(len: usize) -> String {
    let (em, en) = if len.is_multiple_of(3) {
        (len / 3, 0)
    } else if len.is_multiple_of(2) {
        (0, len / 2)
    } else {
        let en = if len % 3 == 1 { 2 } else { 1 };
        ((len - 2 * en) / 3, en)
    };
    let mut out = String::with_capacity((em + en) * 3);
    out.extend(std::iter::repeat_n('\u{2014}', em));
    out.extend(std::iter::repeat_n('\u{2013}', en));
    out
}

/// Fold a run of `len` dots into one ellipsis (`…`) per group of three, leaving the remaining one or
/// two dots literal.
pub(crate) fn fold_ellipsis_run(len: usize) -> String {
    let mut out = String::with_capacity(len / 3 * 3 + len % 3);
    out.extend(std::iter::repeat_n('\u{2026}', len / 3));
    out.extend(std::iter::repeat_n('.', len % 3));
    out
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
