//! Shared helpers for the text-oriented writers: the default fill column, the greedy line-filling
//! engine, column-width measurement, list-tightness, ordered-list numerals and delimiter wrapping,
//! the smart-quote glyphs, URI-scheme recognition, and HTML attribute and entity helpers.
//!
//! Each consumer is behind its own writer feature, so which helpers are live depends on the enabled
//! features: a build with only one writer leaves the others' helpers unreferenced. That is expected
//! for this toolbox, so unused-item warnings are allowed here rather than gated per item.
#![allow(dead_code)]

use oxidoc_ast::{Attr, Block, ListNumberDelim, ListNumberStyle, QuoteType};

/// Column at which inline content is wrapped: the default fill width.
pub(crate) const FILL_COLUMN: usize = 72;

/// The open and close smart-quote glyphs for a quote kind.
pub(crate) fn quote_marks(kind: &QuoteType) -> (char, char) {
    match kind {
        QuoteType::SingleQuote => ('\u{2018}', '\u{2019}'),
        QuoteType::DoubleQuote => ('\u{201c}', '\u{201d}'),
    }
}

/// A unit of inline content awaiting line filling: an unbreakable text run, a breakable space, or a
/// forced line break.
#[derive(Debug, Clone)]
pub(crate) enum Piece {
    Text(String),
    Space,
    Hard,
}

/// Greedily fill inline pieces to `width` columns: a breakable space becomes a line break when
/// keeping the next word on the current line would exceed the fill column. Consecutive text runs (no
/// intervening space) stay together; runs of spaces collapse; leading and trailing spaces on a line
/// are dropped.
pub(crate) fn fill(pieces: &[Piece], width: usize) -> String {
    fill_offset(pieces, width, 0)
}

/// Like [`fill`], but the first line is laid out as if `initial` columns were already consumed (the
/// hanging-marker layout, where a leading marker shifts the first line's wrap point but leaves
/// continuation lines at the margin).
pub(crate) fn fill_offset(pieces: &[Piece], width: usize, initial: usize) -> String {
    let width = width.max(1);
    let mut out = String::new();
    let mut column = initial;
    let mut at_line_start = initial == 0;
    let mut pending_space = false;
    // Consecutive text pieces (no intervening space or break) form one unbreakable word, gathered
    // here as borrowed runs and placed only once its full width is known.
    let mut word: Vec<&str> = Vec::new();
    let mut word_width = 0;
    for piece in pieces {
        match piece {
            Piece::Text(text) => {
                word.push(text);
                word_width += display_width(text);
            }
            Piece::Space => {
                place_word(
                    &mut out,
                    &mut column,
                    &mut at_line_start,
                    pending_space,
                    &word,
                    word_width,
                    width,
                );
                word.clear();
                word_width = 0;
                pending_space = true;
            }
            Piece::Hard => {
                place_word(
                    &mut out,
                    &mut column,
                    &mut at_line_start,
                    pending_space,
                    &word,
                    word_width,
                    width,
                );
                word.clear();
                word_width = 0;
                if !at_line_start {
                    out.push('\n');
                    column = 0;
                    at_line_start = true;
                }
                pending_space = false;
            }
        }
    }
    place_word(
        &mut out,
        &mut column,
        &mut at_line_start,
        pending_space,
        &word,
        word_width,
        width,
    );
    out.trim_end_matches('\n').to_owned()
}

/// Place a gathered word onto the current line, inserting a line break in place of the preceding
/// space when keeping the word would overflow `width`. A no-op for an empty word.
fn place_word(
    out: &mut String,
    column: &mut usize,
    at_line_start: &mut bool,
    pending_space: bool,
    word: &[&str],
    word_width: usize,
    width: usize,
) {
    if word.is_empty() {
        return;
    }
    if *at_line_start {
        *at_line_start = false;
    } else if pending_space && *column + 1 + word_width > width {
        out.push('\n');
        *column = 0;
        *at_line_start = false;
    } else if pending_space {
        out.push(' ');
        *column += 1;
    }
    for part in word {
        out.push_str(part);
    }
    *column += word_width;
}

/// Apply `first` to the first line and `rest` to each non-empty later line, leaving blank lines
/// (block separators) unprefixed. This produces a hanging indent: a list marker plus continuation
/// indent, or a uniform block-quote / code prefix.
pub(crate) fn indent_block(body: &str, first: &str, rest: &str) -> String {
    let mut out = String::new();
    for (index, line) in body.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        if index == 0 {
            out.push_str(first);
            out.push_str(line);
        } else if !line.is_empty() {
            out.push_str(rest);
            out.push_str(line);
        }
    }
    out
}

/// Whether a list is tight: every item is empty or opens with a [`Block::Plain`].
pub(crate) fn list_is_tight(items: &[Vec<Block>]) -> bool {
    items
        .iter()
        .all(|item| matches!(item.first(), None | Some(Block::Plain(_))))
}

/// Wrap an ordered-list numeral in its delimiter: `n.`, `n)`, or `(n)`.
pub(crate) fn wrap_delim(numeral: &str, delim: &ListNumberDelim) -> String {
    match delim {
        ListNumberDelim::DefaultDelim | ListNumberDelim::Period => format!("{numeral}."),
        ListNumberDelim::OneParen => format!("{numeral})"),
        ListNumberDelim::TwoParens => format!("({numeral})"),
    }
}

/// Display width of a string in columns, summed over its characters.
pub(crate) fn display_width(text: &str) -> usize {
    text.chars().map(char_width).sum()
}

/// Display width of a character: zero for common combining marks and controls, two for wide East
/// Asian characters, one otherwise. A self-contained column-width approximation.
pub(crate) fn char_width(ch: char) -> usize {
    let code = ch as u32;
    if is_control(code) {
        return 0;
    }
    if code < 0x0300 {
        return 1;
    }
    if is_zero_width(code) {
        return 0;
    }
    if is_wide(code) { 2 } else { 1 }
}

/// C0 controls, DEL, and C1 controls occupy no display columns.
fn is_control(code: u32) -> bool {
    code < 0x20 || (0x7F..=0x9F).contains(&code)
}

fn is_zero_width(code: u32) -> bool {
    matches!(code,
        0x0300..=0x036F
        | 0x0483..=0x0489
        | 0x0591..=0x05BD
        | 0x0610..=0x061A
        | 0x064B..=0x065F
        | 0x0670
        | 0x06D6..=0x06DC
        | 0x06DF..=0x06E4
        | 0x0E31
        | 0x0E34..=0x0E3A
        | 0x1AB0..=0x1AFF
        | 0x1DC0..=0x1DFF
        | 0x200B..=0x200F
        | 0x20D0..=0x20FF
        | 0xFE00..=0xFE0F
        | 0xFE20..=0xFE2F
    )
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

/// Convert a zero-based item offset to the signed step added to a list's start number, saturating an
/// out-of-range offset rather than overflowing.
pub(crate) fn offset_as_i32(offset: usize) -> i32 {
    i32::try_from(offset).unwrap_or(i32::MAX)
}

/// The leading marker for an ordered-list item: its number in the list's numeral style, wrapped in
/// the list's delimiter.
pub(crate) fn ordered_marker(
    number: i32,
    style: &ListNumberStyle,
    delim: &ListNumberDelim,
) -> String {
    wrap_delim(&numeral(number, style), delim)
}

/// Render a number in a list's numeral style.
pub(crate) fn numeral(number: i32, style: &ListNumberStyle) -> String {
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

/// Look up a key/value attribute by key, returning its value.
pub(crate) fn attribute_value<'a>(attr: &'a Attr, key: &str) -> Option<&'a str> {
    attr.attributes
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

/// Whether a string is syntactically a URI scheme: an ASCII letter followed by ASCII letters,
/// digits, or any of `+`, `-`, `.`.
pub(crate) fn is_uri_scheme(scheme: &str) -> bool {
    let mut chars = scheme.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'))
}

/// Escape the XML/HTML metacharacters `&`, `<`, and `>` to their entities, and additionally `"` when
/// `escape_quotes` is set (as in an attribute value).
pub(crate) fn escape_xml(text: &str, escape_quotes: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if escape_quotes => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
    out
}

/// Whether an attribute name is emitted verbatim in HTML output. Recognized names, the `data-`/`aria-`
/// prefixes, and a few namespaced names pass through; any other key/value attribute is `data-`
/// prefixed by the caller.
pub(crate) fn is_known_attribute(name: &str) -> bool {
    name.starts_with("data-")
        || name.starts_with("aria-")
        || matches!(name, "epub:type" | "xml:lang" | "xmlns")
        || HTML_ATTRIBUTES.contains(&name)
}

/// HTML attribute names emitted verbatim; any other key/value attribute is `data-` prefixed.
const HTML_ATTRIBUTES: &[&str] = &[
    "abbr",
    "accept",
    "accept-charset",
    "accesskey",
    "action",
    "allow",
    "alt",
    "async",
    "autocapitalize",
    "autocomplete",
    "autofocus",
    "autoplay",
    "charset",
    "checked",
    "cite",
    "class",
    "cols",
    "colspan",
    "content",
    "contenteditable",
    "controls",
    "coords",
    "crossorigin",
    "data",
    "datetime",
    "decoding",
    "default",
    "defer",
    "dir",
    "dirname",
    "disabled",
    "download",
    "draggable",
    "enctype",
    "enterkeyhint",
    "for",
    "form",
    "formaction",
    "formenctype",
    "formmethod",
    "formnovalidate",
    "formtarget",
    "headers",
    "height",
    "hidden",
    "high",
    "href",
    "hreflang",
    "id",
    "inputmode",
    "integrity",
    "is",
    "ismap",
    "itemid",
    "itemprop",
    "itemref",
    "itemscope",
    "itemtype",
    "kind",
    "lang",
    "list",
    "loading",
    "loop",
    "low",
    "max",
    "maxlength",
    "media",
    "method",
    "min",
    "minlength",
    "multiple",
    "muted",
    "name",
    "nonce",
    "novalidate",
    "open",
    "optimum",
    "pattern",
    "ping",
    "placeholder",
    "playsinline",
    "poster",
    "preload",
    "readonly",
    "referrerpolicy",
    "rel",
    "required",
    "reversed",
    "role",
    "rows",
    "rowspan",
    "sandbox",
    "scope",
    "selected",
    "shape",
    "size",
    "sizes",
    "slot",
    "span",
    "spellcheck",
    "src",
    "srcdoc",
    "srcset",
    "start",
    "step",
    "style",
    "tabindex",
    "target",
    "title",
    "translate",
    "type",
    "usemap",
    "value",
    "width",
    "wrap",
];
