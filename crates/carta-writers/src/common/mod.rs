//! Shared helpers for the text-oriented writers, organized into cohesive submodules: the line-fill
//! engine ([`fill`]), column-width measurement ([`width`]), list-tightness and numerals ([`lists`]),
//! footnote plumbing ([`notes`]), image-dimension and attribute helpers ([`dimensions`]), URI-scheme
//! recognition ([`uri`]), markup escaping and HTML attributes ([`html_attr`]), the table grid model
//! ([`tables`]), and text-grid table layout ([`table_layout`]). The submodule layout is a private
//! organizational detail — every item is re-exported here, so consumers import from `crate::common`
//! rather than reaching into a submodule.
//!
//! This root also holds a few cross-cutting helpers (smart-quote glyphs, ASCII punctuation fallback,
//! list-offset conversion, raw-passthrough emission). Which of these is live depends on the enabled
//! writer features, so unused-item warnings are allowed here rather than gated per item; likewise a
//! single-writer build leaves whole submodule re-exports unreferenced, so unused-import warnings on
//! the glob re-exports are allowed too.
#![allow(dead_code, unused_imports)]

mod dimensions;
mod fill;
mod html_attr;
mod lists;
mod notes;
#[cfg(any(feature = "plain", feature = "markdown", feature = "gfm"))]
mod table_layout;
mod tables;
mod uri;
mod width;

pub(crate) use dimensions::*;
pub(crate) use fill::*;
pub(crate) use html_attr::*;
pub(crate) use lists::*;
pub(crate) use notes::*;
#[cfg(any(feature = "plain", feature = "markdown", feature = "gfm"))]
pub(crate) use table_layout::*;
pub(crate) use tables::*;
pub(crate) use uri::*;
pub(crate) use width::*;

use carta_ast::QuoteType;

/// The open and close smart-quote glyphs for a quote kind.
pub(crate) fn quote_marks(kind: &QuoteType) -> (char, char) {
    match kind {
        QuoteType::SingleQuote => ('\u{2018}', '\u{2019}'),
        QuoteType::DoubleQuote => ('\u{201c}', '\u{201d}'),
    }
}

/// The ASCII rendering of a Unicode smart-punctuation character: the form a `smart`-enabled writer
/// emits so the text round-trips through a non-Unicode reader. Returns `None` for any other char.
/// Curly quotes collapse to straight quotes, en/em dashes to `--`/`---`, and the ellipsis to `...`.
pub(crate) fn ascii_punctuation(ch: char) -> Option<&'static str> {
    Some(match ch {
        '\u{2018}' | '\u{2019}' => "'",
        '\u{201c}' | '\u{201d}' => "\"",
        '\u{2013}' => "--",
        '\u{2014}' => "---",
        '\u{2026}' => "...",
        _ => return None,
    })
}

/// Convert a zero-based item offset to the signed step added to a list's start number, saturating an
/// out-of-range offset rather than overflowing.
pub(crate) fn offset_as_i32(offset: usize) -> i32 {
    i32::try_from(offset).unwrap_or(i32::MAX)
}

/// Byte length of the longest prefix of `text` that contains no trigger byte, per `is_trigger`. An
/// escaper copies this prefix verbatim and resumes its per-character work at the first trigger.
///
/// `is_trigger` must classify bytes so the returned length always falls on a `char` boundary: match
/// only ASCII bytes (`< 0x80`), or — for an escaper that acts on non-ASCII input — additionally
/// treat *every* byte `>= 0x80` as a trigger. Under that contract the first trigger byte is either an
/// ASCII byte or the leading byte of a multibyte character, never a continuation byte.
pub(crate) fn clean_prefix_len(text: &str, is_trigger: impl Fn(u8) -> bool) -> usize {
    text.as_bytes()
        .iter()
        .position(|&byte| is_trigger(byte))
        .unwrap_or(text.len())
}

/// How a raw-passthrough payload's trailing newlines are handled before emission.
#[derive(Clone, Copy)]
pub(crate) enum RawTrim {
    /// Emit the payload verbatim.
    Keep,
    /// Drop a single trailing newline.
    DropOne,
    /// Drop every trailing newline.
    DropAll,
}

/// Emit a raw block or inline payload only when its format names this writer's token, otherwise drop
/// it. Recognition is case-insensitive. The trailing-newline policy is the writer's own.
pub(crate) fn raw_passthrough(
    format: &carta_ast::Format,
    text: &str,
    token: &str,
    trim: RawTrim,
) -> String {
    if !format.0.eq_ignore_ascii_case(token) {
        return String::new();
    }
    match trim {
        RawTrim::Keep => text.to_owned(),
        RawTrim::DropOne => text.strip_suffix('\n').unwrap_or(text).to_owned(),
        RawTrim::DropAll => text.trim_end_matches('\n').to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_conversion_saturates() {
        assert_eq!(offset_as_i32(0), 0);
        assert_eq!(offset_as_i32(7), 7);
        assert_eq!(offset_as_i32(usize::MAX), i32::MAX);
    }

    #[test]
    fn quote_marks_per_kind() {
        assert_eq!(
            quote_marks(&QuoteType::SingleQuote),
            ('\u{2018}', '\u{2019}')
        );
        assert_eq!(
            quote_marks(&QuoteType::DoubleQuote),
            ('\u{201c}', '\u{201d}')
        );
    }
}
