//! Shared helpers for the text-oriented writers (plain and HTML): the default fill column, the
//! East-Asian wide-character measure, and the smart-quote glyphs.

use oxidoc_ast::QuoteType;

/// Column at which inline content is wrapped, matching the reference writer's default fill width.
pub(crate) const FILL_COLUMN: usize = 72;

/// The open and close smart-quote glyphs for a quote kind.
pub(crate) fn quote_marks(kind: &QuoteType) -> (char, char) {
    match kind {
        QuoteType::SingleQuote => ('\u{2018}', '\u{2019}'),
        QuoteType::DoubleQuote => ('\u{201c}', '\u{201d}'),
    }
}

/// Whether a character occupies two display columns: the wide and fullwidth East Asian ranges.
pub(crate) fn is_wide(code: u32) -> bool {
    matches!(code,
        0x1100..=0x115F
        | 0x2329 | 0x232A
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xA960..=0xA97F
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE10..=0xFE19
        | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1B000..=0x1B2FF
        | 0x1F200..=0x1F2FF
        | 0x1F300..=0x1F64F
        | 0x1F900..=0x1F9FF
        | 0x20000..=0x3FFFD
    )
}
