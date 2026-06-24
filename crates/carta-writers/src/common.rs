//! Shared helpers for the text-oriented writers: the default fill column, the greedy line-filling
//! engine, column-width measurement, list-tightness, ordered-list numerals and delimiter wrapping,
//! the smart-quote glyphs, URI-scheme recognition, and HTML attribute and entity helpers.
//!
//! Each consumer is behind its own writer feature, so which helpers are live depends on the enabled
//! features: a build with only one writer leaves the others' helpers unreferenced. That is expected
//! for this toolbox, so unused-item warnings are allowed here rather than gated per item.
#![allow(dead_code)]

#[cfg(any(feature = "plain", feature = "markdown", feature = "gfm"))]
use carta_ast::{Alignment, Cell, ColWidth, Row, Table};
use carta_ast::{Attr, Block, Inline, ListNumberDelim, ListNumberStyle, QuoteType};
use carta_core::WrapMode;

/// Column at which inline content is wrapped: the default fill width.
pub(crate) const FILL_COLUMN: usize = 72;

/// The open and close smart-quote glyphs for a quote kind.
pub(crate) fn quote_marks(kind: &QuoteType) -> (char, char) {
    match kind {
        QuoteType::SingleQuote => ('\u{2018}', '\u{2019}'),
        QuoteType::DoubleQuote => ('\u{201c}', '\u{201d}'),
    }
}

/// A unit of inline content awaiting line filling: an unbreakable text run, a breakable space, a
/// soft line break from the source, or a forced line break.
#[derive(Debug, Clone)]
pub(crate) enum Piece {
    Text(String),
    Space,
    /// A soft line break in the source. Under [`WrapMode::Preserve`] it stays a line break; under
    /// [`WrapMode::Auto`] and [`WrapMode::None`] it is inter-word space like [`Piece::Space`].
    Soft,
    Hard,
}

/// Greedily fill inline pieces to `width` columns: a breakable space becomes a line break when
/// keeping the next word on the current line would exceed the fill column. Consecutive text runs (no
/// intervening space) stay together; runs of spaces collapse; leading and trailing spaces on a line
/// are dropped.
///
/// The `wrap` mode governs line layout: [`WrapMode::Auto`] reflows to `width`; [`WrapMode::None`]
/// never wraps (the whole paragraph is one line, soft breaks becoming spaces); [`WrapMode::Preserve`]
/// does not reflow but keeps each soft break from the source as a line break.
pub(crate) fn fill(pieces: &[Piece], width: usize, wrap: WrapMode) -> String {
    fill_offset(pieces, width, 0, wrap)
}

/// Like [`fill`], but the first line is laid out as if `initial` columns were already consumed (the
/// hanging-marker layout, where a leading marker shifts the first line's wrap point but leaves
/// continuation lines at the margin).
pub(crate) fn fill_offset(
    pieces: &[Piece],
    width: usize,
    initial: usize,
    wrap: WrapMode,
) -> String {
    // Auto reflows to the fill column; the other modes never wrap on width, so a sentinel column
    // wide enough that no real line reaches it stands in. Soft breaks are the only line splits then.
    let width = match wrap {
        WrapMode::Auto => width.max(1),
        WrapMode::None | WrapMode::Preserve => usize::MAX,
    };
    fill_core(pieces, width, initial, matches!(wrap, WrapMode::Preserve))
}

/// Lay out a table cell's inline content to a fixed-width column field. Unlike [`fill`], the field
/// reflows to `width` under both [`WrapMode::Auto`] and [`WrapMode::Preserve`] — a bordered cell is
/// always bounded by its column — while [`WrapMode::None`] still renders the content on one line so
/// the column can instead grow to hold it. Under [`WrapMode::Preserve`] each source soft break stays
/// a forced line break, with the text between breaks reflowed to the field width.
pub(crate) fn fill_cell(pieces: &[Piece], width: usize, wrap: WrapMode) -> String {
    let width = match wrap {
        WrapMode::None => usize::MAX,
        WrapMode::Auto | WrapMode::Preserve => width.max(1),
    };
    fill_core(pieces, width, 0, matches!(wrap, WrapMode::Preserve))
}

/// The shared line-filling engine behind [`fill_offset`] and [`fill_cell`]: lay `pieces` out into
/// lines no wider than `width` (already resolved to a sentinel when the caller wants no width wrap),
/// starting `initial` columns into the first line, breaking on each source soft break only when
/// `preserve_softs` is set.
fn fill_core(pieces: &[Piece], width: usize, initial: usize, preserve_softs: bool) -> String {
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
            // A soft break forces a line break only when preserving the source's own breaks;
            // otherwise it is just inter-word space (and may become a reflow point under Auto).
            Piece::Soft if preserve_softs => {
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
            Piece::Space | Piece::Soft => {
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

/// A text writer that gathers footnotes inline and emits them as a trailing section. Each note is
/// referenced by a numbered `[n]` marker; its body is rendered offset so the marker shifts only the
/// first line's wrap point. The format supplies how a block and a marker-offset leading paragraph
/// render; the marker numbering and slot bookkeeping are shared here.
pub(crate) trait NotesHost {
    /// The accumulated note bodies, indexed by note number minus one.
    fn notes(&mut self) -> &mut Vec<String>;

    /// Render a block at the given fill width.
    fn render_block(&mut self, block: &Block, width: usize) -> String;

    /// Render a leading paragraph's text with its first line beginning `initial` columns in.
    fn render_offset_paragraph(
        &mut self,
        inlines: &[Inline],
        width: usize,
        initial: usize,
    ) -> String;

    /// The fill width a note's body lays out to: the document's configured column width.
    fn base_width(&self) -> usize {
        FILL_COLUMN
    }

    /// Record a footnote: reserve its slot before rendering (so nested notes number after it), fill
    /// the slot with the assembled body, and return the inline `[n]` marker.
    fn record_note(&mut self, blocks: &[Block]) -> String {
        self.numbered_note(blocks)
    }

    /// Record a footnote in the generic numbered form — a `[n]` reference marker and a matching
    /// `[n] body` definition whose first line is offset by the marker width. This is the layout a
    /// markdown dialect without the `footnotes` extension falls back to, so it stays reachable even
    /// when [`record_note`](Self::record_note) is overridden with a richer footnote syntax.
    fn numbered_note(&mut self, blocks: &[Block]) -> String {
        let index = self.notes().len();
        self.notes().push(String::new());
        let marker = format!("[{}]", index + 1);
        let field = marker.chars().count() + 1;
        let body = self.offset_note_body(blocks, field);
        // The body shares the marker's line only when it opens with a paragraph; a leading block of
        // any other kind (a code block, a list) begins on the line below the marker.
        let starts_inline = matches!(blocks.first(), Some(Block::Plain(_) | Block::Para(_)));
        let rendered = if body.is_empty() {
            marker.clone()
        } else if starts_inline {
            format!("{marker} {body}")
        } else {
            format!("{marker}\n{body}")
        };
        if let Some(slot) = self.notes().get_mut(index) {
            *slot = rendered;
        }
        marker
    }

    /// Render a footnote's body: the first block's opening line is offset by the marker width, every
    /// later block and continuation line sits at the margin.
    fn note_body(&mut self, blocks: &[Block], initial: usize) -> String {
        self.offset_note_body(blocks, initial)
    }

    /// The marker-offset note body: the first block's opening line begins `initial` columns in and
    /// every later block sits at the margin. Kept separate from [`note_body`](Self::note_body) so an
    /// overriding writer can still reach the generic layout from [`numbered_note`](Self::numbered_note).
    fn offset_note_body(&mut self, blocks: &[Block], initial: usize) -> String {
        let width = self.base_width();
        let rendered = blocks
            .iter()
            .enumerate()
            .map(|(position, block)| {
                let is_plain = matches!(block, Block::Plain(_));
                let text = if position == 0 {
                    self.note_block_offset(block, width, initial)
                } else {
                    self.render_block(block, width)
                };
                (is_plain, text)
            })
            .collect();
        join_loose(rendered)
    }

    /// Render a block whose first line begins `initial` columns in. Only a leading paragraph wraps,
    /// so the offset is meaningful for it alone; other block kinds render at the margin.
    fn note_block_offset(&mut self, block: &Block, width: usize, initial: usize) -> String {
        match block {
            Block::Plain(inlines) | Block::Para(inlines) => {
                self.render_offset_paragraph(inlines, width, initial)
            }
            other => self.render_block(other, width),
        }
    }
}

/// Append a gathered footnote section to a rendered body, separated by a blank line, and trim the
/// trailing newlines. With no notes this just trims the body.
pub(crate) fn append_notes(body: String, notes: &[String]) -> String {
    let mut out = body;
    if !notes.is_empty() {
        let section = notes.join("\n\n");
        out = if out.is_empty() {
            section
        } else {
            format!("{out}\n\n{section}")
        };
    }
    out.trim_end_matches('\n').to_owned()
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

/// How a raw-passthrough payload's trailing newlines are handled before emission.
#[cfg(any(
    feature = "dokuwiki",
    feature = "jira",
    feature = "man",
    feature = "asciidoc"
))]
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
#[cfg(any(
    feature = "dokuwiki",
    feature = "jira",
    feature = "man",
    feature = "asciidoc"
))]
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

/// Look up a key/value attribute by key, returning its value.
pub(crate) fn attribute_value<'a>(attr: &'a Attr, key: &str) -> Option<&'a str> {
    attr.attributes
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

/// A parsed image dimension: a pixel count rendered as a bare HTML attribute, or a length rendered
/// inside a CSS `style` declaration.
#[cfg(feature = "html")]
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Dimension {
    /// A pixel or unitless value, truncated to a whole number and emitted as a bare `width`/`height`
    /// attribute.
    Pixels(u64),
    /// A percentage, emitted in a `style` declaration with one-decimal formatting (`50.0%`).
    Percent(f64),
    /// A physical or font-relative length, emitted in a `style` declaration; the numeric part is
    /// rounded to five fractional digits with trailing zeros dropped, the unit kept verbatim.
    Length(f64, &'static str),
}

/// The length units accepted in an image dimension. Each entry pairs the spelling that may appear in
/// the source value with the unit emitted in the `style` declaration (`inch` normalizes to `in`).
#[cfg(feature = "html")]
const DIMENSION_UNITS: &[(&str, &str)] = &[
    ("cm", "cm"),
    ("mm", "mm"),
    ("in", "in"),
    ("inch", "in"),
    ("pt", "pt"),
    ("pc", "pc"),
    ("em", "em"),
];

/// Parse an image `width`/`height` value into a [`Dimension`], or `None` when it is not a recognized
/// dimension (an unknown unit, a malformed or signed number, surrounding whitespace), in which case
/// the attribute is dropped. The numeric part is a run of digits with an optional single fractional
/// part, no sign and no surrounding space; a bare number or a `px` suffix is a pixel count, `%` a
/// percentage, and a unit from [`DIMENSION_UNITS`] a physical length.
#[cfg(feature = "html")]
pub(crate) fn parse_dimension(value: &str) -> Option<Dimension> {
    if let Some(number) = value.strip_suffix('%') {
        let magnitude = parse_dimension_number(number)?;
        return Some(Dimension::Percent(magnitude));
    }
    if let Some(number) = value.strip_suffix("px") {
        let magnitude = parse_dimension_number(number)?;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        return Some(Dimension::Pixels(magnitude.trunc() as u64));
    }
    for (spelling, unit) in DIMENSION_UNITS {
        if let Some(number) = value.strip_suffix(spelling) {
            let magnitude = parse_dimension_number(number)?;
            return Some(Dimension::Length(magnitude, unit));
        }
    }
    let magnitude = parse_dimension_number(value)?;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some(Dimension::Pixels(magnitude.trunc() as u64))
}

/// Parse the numeric part of a dimension: a run of ASCII digits with an optional single fractional
/// run (`123` or `123.45`), no sign, no leading or trailing dot, no surrounding whitespace. Returns
/// the magnitude, or `None` for any other shape.
#[cfg(feature = "html")]
fn parse_dimension_number(text: &str) -> Option<f64> {
    let (whole, fraction) = match text.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (text, None),
    };
    let is_digits = |run: &str| !run.is_empty() && run.bytes().all(|byte| byte.is_ascii_digit());
    if !is_digits(whole) {
        return None;
    }
    if let Some(fraction) = fraction
        && !is_digits(fraction)
    {
        return None;
    }
    text.parse::<f64>().ok().filter(|value| value.is_finite())
}

/// Render the CSS value of a percentage dimension: the magnitude with at least one fractional digit
/// (`50` renders as `50.0`), kept to its shortest round-tripping form otherwise.
#[cfg(feature = "html")]
pub(crate) fn format_percent_dimension(magnitude: f64) -> String {
    format!("{magnitude:?}%")
}

/// Render the CSS value of a length dimension: the magnitude rounded to five fractional digits with
/// trailing zeros (and a bare trailing dot) dropped, followed by the unit.
#[cfg(feature = "html")]
pub(crate) fn format_length_dimension(magnitude: f64, unit: &str) -> String {
    let rounded = (magnitude * 100_000.0).round() / 100_000.0;
    let mut number = format!("{rounded:.5}");
    if number.contains('.') {
        let trimmed = number.trim_end_matches('0').trim_end_matches('.');
        number.truncate(trimmed.len());
    }
    format!("{number}{unit}")
}

/// Normalize an image's attributes so `width` and `height` render as an HTML `<img>` does: a pixel or
/// unitless value becomes a bare numeric attribute (the `px` stripped), a percentage or physical
/// length folds into a CSS `style` declaration. An unrecognized or malformed value is dropped.
///
/// The resulting key/value order is: the combined `style` (any source `style` value followed by the
/// `width` then `height` style declarations) first, then the remaining attributes in source order
/// with `width`/`height`/`style` removed, then the pixel `width` and `height` attributes last. The
/// `id` and `class` carry over unchanged. A writer renders the returned [`Attr`] with its own
/// attribute renderer, so the dimension rule lives here alone.
#[cfg(feature = "html")]
pub(crate) fn normalize_image_attr(attr: &Attr) -> Attr {
    let mut base_style: Option<String> = None;
    let mut style_declarations: Vec<String> = Vec::new();
    let mut pixel_attrs: Vec<(String, String)> = Vec::new();
    let mut rest: Vec<(String, String)> = Vec::new();

    let emit_dimension = |key: &str,
                          raw: &str,
                          declarations: &mut Vec<String>,
                          pixels: &mut Vec<(String, String)>| {
        match parse_dimension(raw) {
            Some(Dimension::Pixels(count)) => pixels.push((key.to_owned(), count.to_string())),
            Some(Dimension::Percent(magnitude)) => {
                declarations.push(format!("{key}:{}", format_percent_dimension(magnitude)));
            }
            Some(Dimension::Length(magnitude, unit)) => {
                declarations.push(format!(
                    "{key}:{}",
                    format_length_dimension(magnitude, unit)
                ));
            }
            None => {}
        }
    };

    // Width and height are emitted in a fixed order regardless of their source position; gather them
    // by lookup so a height-before-width source still renders width first.
    if let Some(raw) = attribute_value(attr, "width") {
        emit_dimension("width", raw, &mut style_declarations, &mut pixel_attrs);
    }
    if let Some(raw) = attribute_value(attr, "height") {
        emit_dimension("height", raw, &mut style_declarations, &mut pixel_attrs);
    }

    for (key, value) in &attr.attributes {
        match key.as_str() {
            "width" | "height" => {}
            "style" => base_style = Some(value.clone()),
            _ => rest.push((key.clone(), value.clone())),
        }
    }

    let style = combine_dimension_style(base_style, &style_declarations);

    let mut attributes = Vec::new();
    if let Some(style) = style {
        attributes.push(("style".to_owned(), style));
    }
    attributes.extend(rest);
    attributes.extend(pixel_attrs);

    Attr {
        id: attr.id.clone(),
        classes: attr.classes.clone(),
        attributes,
    }
}

/// Combine a source `style` value with the dimension style declarations: a present source value is
/// kept verbatim and the declarations appended after a `;`; with no source style the declarations
/// join with `;`. Yields `None` when there is nothing to emit.
#[cfg(feature = "html")]
fn combine_dimension_style(base: Option<String>, declarations: &[String]) -> Option<String> {
    let joined = declarations.join(";");
    match base {
        Some(base) if joined.is_empty() => Some(base),
        Some(base) => Some(format!("{base};{joined}")),
        None if joined.is_empty() => None,
        None => Some(joined),
    }
}

/// Split a CSS length into its leading numeric run (digits, `.`, sign) and the trailing unit. A
/// value with no unit yields an empty unit; a value with no numeric prefix yields an empty number.
#[cfg(any(feature = "dokuwiki", feature = "asciidoc"))]
pub(crate) fn split_length_unit(raw: &str) -> (&str, &str) {
    let boundary = raw
        .find(|ch: char| !(ch.is_ascii_digit() || ch == '.' || ch == '-' || ch == '+'))
        .unwrap_or(raw.len());
    (
        raw.get(..boundary).unwrap_or(raw),
        raw.get(boundary..).unwrap_or(""),
    )
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

/// Whether `scheme` names a registered URI scheme (compared case-insensitively), per the
/// [`URI_SCHEMES`] registry. Used to decide whether an address may render as a bare autolink.
pub(crate) fn is_known_scheme(scheme: &str) -> bool {
    let lowered = scheme.to_ascii_lowercase();
    URI_SCHEMES.binary_search(&lowered.as_str()).is_ok()
}

/// Registered URI schemes, lowercased and sorted for binary search. Drawn from the IANA scheme
/// registry; the autolink-capable writers share this one set so their recognition cannot drift.
const URI_SCHEMES: &[&str] = &[
    "aaa",
    "aaas",
    "about",
    "acap",
    "acct",
    "acr",
    "adiumxtra",
    "admin",
    "afp",
    "afs",
    "aim",
    "app",
    "appdata",
    "apt",
    "attachment",
    "aw",
    "barion",
    "beshare",
    "bitcoin",
    "bitcoincash",
    "blob",
    "bolo",
    "browserext",
    "bzr",
    "callto",
    "cap",
    "chrome",
    "chrome-extension",
    "cid",
    "coap",
    "coaps",
    "com-eventbrite-attendee",
    "content",
    "crid",
    "cvs",
    "data",
    "dav",
    "dict",
    "did",
    "dis",
    "dlna-playcontainer",
    "dlna-playsingle",
    "dns",
    "dntp",
    "doi",
    "dtn",
    "dvb",
    "ed2k",
    "ethereum",
    "example",
    "facetime",
    "fax",
    "feed",
    "feedready",
    "file",
    "filesystem",
    "finger",
    "fish",
    "ftp",
    "geo",
    "gg",
    "git",
    "gizmoproject",
    "go",
    "gopher",
    "graph",
    "gtalk",
    "h323",
    "ham",
    "hcp",
    "http",
    "https",
    "hxxp",
    "hxxps",
    "hydrazone",
    "iax",
    "icap",
    "icon",
    "im",
    "imap",
    "info",
    "iotdisco",
    "ipn",
    "ipp",
    "ipps",
    "irc",
    "irc6",
    "ircs",
    "iris",
    "iris.beep",
    "iris.lwz",
    "iris.xpc",
    "iris.xpcs",
    "isostore",
    "itms",
    "jabber",
    "jar",
    "jms",
    "keyparc",
    "lastfm",
    "ldap",
    "ldaps",
    "lvlt",
    "magnet",
    "mailserver",
    "mailto",
    "maps",
    "market",
    "matrix",
    "message",
    "mid",
    "mms",
    "modem",
    "monero",
    "mongodb",
    "moz",
    "ms-access",
    "ms-browser-extension",
    "ms-drive-to",
    "ms-enrollment",
    "ms-excel",
    "ms-gamebarservices",
    "ms-getoffice",
    "ms-help",
    "ms-infopath",
    "ms-media-stream-id",
    "ms-officeapp",
    "ms-powerpoint",
    "ms-project",
    "ms-publisher",
    "ms-search-repair",
    "ms-secondary-screen-controller",
    "ms-secondary-screen-setup",
    "ms-settings",
    "ms-settings-airplanemode",
    "ms-settings-bluetooth",
    "ms-settings-camera",
    "ms-settings-cellular",
    "ms-settings-cloudstorage",
    "ms-settings-connectabledevices",
    "ms-settings-displays-topology",
    "ms-settings-emailandaccounts",
    "ms-settings-language",
    "ms-settings-location",
    "ms-settings-lock",
    "ms-settings-nfctransactions",
    "ms-settings-notifications",
    "ms-settings-power",
    "ms-settings-privacy",
    "ms-settings-proximity",
    "ms-settings-screenrotation",
    "ms-settings-wifi",
    "ms-settings-workplace",
    "ms-spd",
    "ms-sttoverlay",
    "ms-transit-to",
    "ms-virtualtouchpad",
    "ms-visio",
    "ms-walk-to",
    "ms-whiteboard",
    "ms-whiteboard-cmd",
    "ms-word",
    "msnim",
    "msrp",
    "msrps",
    "mtqp",
    "mumble",
    "mupdate",
    "mvn",
    "mvrp",
    "news",
    "nfs",
    "ni",
    "nih",
    "nntp",
    "notes",
    "ocf",
    "oid",
    "onenote",
    "onenote-cmd",
    "opaquelocktoken",
    "pack",
    "palm",
    "paparazzi",
    "payto",
    "pkcs11",
    "platform",
    "pop",
    "pres",
    "prospero",
    "proxy",
    "psyc",
    "pwid",
    "qb",
    "query",
    "redis",
    "rediss",
    "reload",
    "res",
    "resource",
    "rmi",
    "rsync",
    "rtmfp",
    "rtmp",
    "rtsp",
    "rtsps",
    "rtspu",
    "secondlife",
    "service",
    "session",
    "sftp",
    "sgn",
    "shttp",
    "sieve",
    "sip",
    "sips",
    "skype",
    "smb",
    "sms",
    "smtp",
    "snews",
    "snmp",
    "soap.beep",
    "soap.beeps",
    "soldat",
    "spotify",
    "ssh",
    "steam",
    "stun",
    "stuns",
    "submit",
    "svn",
    "tag",
    "teamspeak",
    "tel",
    "teliaeid",
    "telnet",
    "tftp",
    "things",
    "thismessage",
    "tip",
    "tn3270",
    "tool",
    "turn",
    "turns",
    "tv",
    "udp",
    "unreal",
    "urn",
    "ut2004",
    "v-event",
    "vemmi",
    "ventrilo",
    "view-source",
    "vnc",
    "wais",
    "webcal",
    "wpid",
    "ws",
    "wss",
    "wtai",
    "wyciwyg",
    "xcon",
    "xcon-userid",
    "xfire",
    "xmlrpc.beep",
    "xmlrpc.beeps",
    "xmpp",
    "xri",
    "ymsgr",
    "z39.50",
    "z39.50r",
    "z39.50s",
];

/// Whether `text` is made up solely of URI-permitted characters with every `%` introducing a
/// two-digit hex escape. ASCII alphanumerics and the unreserved, sub-delimiter, and generic-delimiter
/// punctuation are permitted; non-ASCII characters are permitted only when `allow_non_ascii` is set.
pub(crate) fn is_percent_escaped_uri(text: &str, allow_non_ascii: bool) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0;
    while let Some(&ch) = chars.get(index) {
        if ch == '%' {
            let two_hex = chars.get(index + 1).is_some_and(char::is_ascii_hexdigit)
                && chars.get(index + 2).is_some_and(char::is_ascii_hexdigit);
            if !two_hex {
                return false;
            }
            index += 3;
            continue;
        }
        if !is_uri_char(ch, allow_non_ascii) {
            return false;
        }
        index += 1;
    }
    true
}

fn is_uri_char(ch: char, allow_non_ascii: bool) -> bool {
    if !ch.is_ascii() {
        return allow_non_ascii;
    }
    ch.is_ascii_alphanumeric()
        || matches!(
            ch,
            '-' | '.'
                | '_'
                | '~'
                | ':'
                | '/'
                | '?'
                | '#'
                | '@'
                | '!'
                | '$'
                | '&'
                | '\''
                | '('
                | ')'
                | '*'
                | '+'
                | ','
                | ';'
                | '='
        )
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

/// Escape an HTML attribute value: `&`, `<`, `>`, and `"` to their entities.
pub(crate) fn escape_attr(text: &str) -> String {
    escape_xml(text, true)
}

/// One column slot of a laid-out row: the start of a cell, or a column covered by a column or row
/// span (or a column the row's cells never reached). A consumer renders a covered slot as its own
/// filler placeholder.
#[cfg(any(
    feature = "html",
    feature = "mediawiki",
    feature = "dokuwiki",
    feature = "jira",
    feature = "man",
    feature = "asciidoc"
))]
pub(crate) enum GridSlot<'cell> {
    Cell(usize, &'cell carta_ast::Cell),
    Covered,
}

/// Resolves each table cell's true starting column within one row group, accounting for cells from
/// earlier rows that still cover columns through their row span. Create one tracker per group of
/// rows a span can extend over (a table head, a body's own head rows, a body's rows, a foot).
#[cfg(any(
    feature = "html",
    feature = "mediawiki",
    feature = "dokuwiki",
    feature = "jira",
    feature = "man",
    feature = "asciidoc"
))]
#[derive(Debug)]
pub(crate) struct RowSpanGrid {
    /// Per column, how many upcoming rows a span opened in an earlier row still covers.
    pending: Vec<i32>,
}

#[cfg(any(
    feature = "html",
    feature = "mediawiki",
    feature = "dokuwiki",
    feature = "jira",
    feature = "man",
    feature = "asciidoc"
))]
impl RowSpanGrid {
    pub(crate) fn new(columns: usize) -> Self {
        Self {
            pending: vec![0; columns],
        }
    }

    /// Place one row's cells: each cell lands on the first column not covered from above and
    /// occupies its column span, and its row span is recorded for the rows that follow. Returns
    /// each cell paired with its starting column.
    pub(crate) fn place<'cells>(
        &mut self,
        cells: &'cells [carta_ast::Cell],
    ) -> Vec<(usize, &'cells carta_ast::Cell)> {
        self.place_slots(cells)
            .into_iter()
            .filter_map(|slot| match slot {
                GridSlot::Cell(column, cell) => Some((column, cell)),
                GridSlot::Covered => None,
            })
            .collect()
    }

    /// Place one row's cells, surfacing every column slot in order: a cell at its starting column,
    /// or a covered placeholder for a column held by a column span, a row span opened above, or the
    /// trailing columns a row span still holds past the row's own cells. Columns the row never
    /// reached (no span covers them) are not emitted; a consumer that lays out a fixed column count
    /// pads those itself.
    pub(crate) fn place_slots<'cells>(
        &mut self,
        cells: &'cells [carta_ast::Cell],
    ) -> Vec<GridSlot<'cells>> {
        let covered: Vec<usize> = self
            .pending
            .iter()
            .enumerate()
            .filter(|(_, rows)| **rows > 0)
            .map(|(column, _)| column)
            .collect();
        let mut slots: Vec<GridSlot<'cells>> = Vec::with_capacity(cells.len());
        let mut column = 0_usize;
        for cell in cells {
            while self.pending.get(column).copied().unwrap_or(0) > 0 {
                slots.push(GridSlot::Covered);
                column = column.saturating_add(1);
            }
            slots.push(GridSlot::Cell(column, cell));
            let col_span = usize::try_from(cell.col_span).unwrap_or(1).max(1);
            let end = column.saturating_add(col_span);
            for _ in 1..col_span {
                slots.push(GridSlot::Covered);
            }
            if self.pending.len() < end {
                self.pending.resize(end, 0);
            }
            for slot in self.pending.iter_mut().take(end).skip(column) {
                *slot = cell.row_span.saturating_sub(1).max(0);
            }
            column = end;
        }
        while self.pending.get(column).copied().unwrap_or(0) > 0 {
            slots.push(GridSlot::Covered);
            column = column.saturating_add(1);
        }
        for column in covered {
            if let Some(rows) = self.pending.get_mut(column) {
                *rows -= 1;
            }
        }
        slots
    }
}

/// Render an [`Attr`] to an HTML attribute string (a leading space per attribute, empty when blank):
/// `id`, then `class`, then key/value pairs, with unrecognized keys `data-` prefixed.
pub(crate) fn render_html_attr(attr: &Attr) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    if !attr.id.is_empty() {
        let _ = write!(out, " id=\"{}\"", escape_attr(&attr.id));
    }
    if !attr.classes.is_empty() {
        let _ = write!(out, " class=\"{}\"", escape_attr(&attr.classes.join(" ")));
    }
    for (key, value) in &attr.attributes {
        let name = if is_known_attribute(key) {
            key.clone()
        } else {
            format!("data-{key}")
        };
        let _ = write!(out, " {name}=\"{}\"", escape_attr(value));
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

/// The inline content of a block, or an empty slice for a block that carries none directly.
#[cfg(any(
    feature = "plain",
    feature = "rst",
    feature = "markdown",
    feature = "gfm"
))]
pub(crate) fn block_inlines(block: &Block) -> &[Inline] {
    match block {
        Block::Plain(inlines) | Block::Para(inlines) => inlines,
        _ => &[],
    }
}

/// Every row of every body, intermediate head rows included, in document order.
#[cfg(any(
    feature = "plain",
    feature = "rst",
    feature = "markdown",
    feature = "gfm"
))]
pub(crate) fn body_rows(table: &carta_ast::Table) -> Vec<&carta_ast::Row> {
    table
        .bodies
        .iter()
        .flat_map(|body| body.head.iter().chain(body.body.iter()))
        .collect()
}

/// Width used to render a grid cell when measuring its natural extent, before column widths are
/// fixed: large enough that no reflow occurs.
#[cfg(any(feature = "plain", feature = "markdown", feature = "gfm"))]
pub(crate) const MEASURE_WIDTH: usize = 100_000;

/// The layout a text-grid table takes: the compact space-aligned simple form, the reflowing
/// multiline form, or the bordered grid form.
#[cfg(any(feature = "plain", feature = "markdown", feature = "gfm"))]
#[derive(Clone, Copy)]
pub(crate) enum TableForm {
    Simple,
    Multiline,
    Grid,
}

/// Choose the rendering form for a table. Spans, block-level cell content, or a footer demand a
/// grid; an explicit column width or a forced break within a cell demands the multiline form;
/// otherwise the compact simple form suffices.
#[cfg(any(feature = "plain", feature = "markdown", feature = "gfm"))]
pub(crate) fn table_form(table: &Table) -> TableForm {
    let rows: Vec<&Row> = table
        .head
        .rows
        .iter()
        .chain(body_rows(table))
        .chain(table.foot.rows.iter())
        .collect();
    let has_span = rows.iter().any(|row| {
        row.cells
            .iter()
            .any(|cell| cell.row_span > 1 || cell.col_span > 1)
    });
    let has_complex = rows
        .iter()
        .any(|row| row.cells.iter().any(|cell| !is_simple_cell(cell)));
    let has_foot = !table.foot.rows.is_empty();
    if has_span || has_complex || has_foot {
        return TableForm::Grid;
    }
    let has_explicit = table
        .col_specs
        .iter()
        .any(|spec| matches!(spec.width, ColWidth::ColWidth(fraction) if fraction > 0.0));
    let has_break = rows.iter().any(|row| row.cells.iter().any(cell_has_break));
    if has_explicit || has_break {
        TableForm::Multiline
    } else {
        TableForm::Simple
    }
}

/// A cell that holds at most one paragraph of inline content, the precondition for the simple and
/// multiline forms.
#[cfg(any(feature = "plain", feature = "markdown", feature = "gfm"))]
pub(crate) fn is_simple_cell(cell: &Cell) -> bool {
    matches!(
        cell.content.as_slice(),
        [] | [Block::Plain(_) | Block::Para(_)]
    )
}

/// The inline content of a simple cell, or an empty slice for anything richer.
#[cfg(any(feature = "plain", feature = "markdown", feature = "gfm"))]
pub(crate) fn cell_inlines(cell: &Cell) -> &[Inline] {
    match cell.content.first() {
        Some(Block::Plain(inlines) | Block::Para(inlines)) => inlines,
        _ => &[],
    }
}

/// Whether a simple cell contains a forced line break, which forces the multiline form.
#[cfg(any(feature = "plain", feature = "markdown", feature = "gfm"))]
pub(crate) fn cell_has_break(cell: &Cell) -> bool {
    is_simple_cell(cell)
        && cell_inlines(cell)
            .iter()
            .any(|inline| matches!(inline, Inline::LineBreak))
}

/// A row of column underlines: a run of dashes per column width, joined by single spaces.
#[cfg(any(feature = "plain", feature = "markdown", feature = "gfm"))]
pub(crate) fn dash_rule(field: &[usize]) -> String {
    field
        .iter()
        .map(|width| "-".repeat(*width))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Pad `text` to `width`, placing the slack according to the column's alignment.
#[cfg(any(feature = "plain", feature = "markdown", feature = "gfm"))]
pub(crate) fn pad_align(text: &str, width: usize, align: &Alignment) -> String {
    let pad = width.saturating_sub(display_width(text));
    match align {
        Alignment::AlignRight => format!("{}{text}", " ".repeat(pad)),
        Alignment::AlignCenter => {
            let left = pad / 2;
            format!("{}{text}{}", " ".repeat(left), " ".repeat(pad - left))
        }
        Alignment::AlignLeft | Alignment::AlignDefault => format!("{text}{}", " ".repeat(pad)),
    }
}

/// Lay a row of already-rendered cells across the column fields, stacking multi-line cells and
/// trimming the trailing edge of each output line.
#[cfg(any(feature = "plain", feature = "markdown", feature = "gfm"))]
pub(crate) fn lay_row(
    cells: &[Vec<String>],
    field: &[usize],
    aligns: &[&Alignment],
) -> Vec<String> {
    let height = cells.iter().map(Vec::len).max().unwrap_or(1).max(1);
    (0..height)
        .map(|line| {
            let mut parts: Vec<String> = Vec::with_capacity(cells.len());
            for (index, cell) in cells.iter().enumerate() {
                let text = cell.get(line).map_or("", String::as_str);
                let width = field.get(index).copied().unwrap_or(0);
                let align = aligns
                    .get(index)
                    .copied()
                    .unwrap_or(&Alignment::AlignDefault);
                parts.push(pad_align(text, width, align));
            }
            if let Some(last) = parts.last_mut() {
                *last = last.trim_end().to_owned();
            }
            parts.join(" ")
        })
        .collect()
}

/// Reflow a row's inline pieces to fill each column, returning the wrapped lines per cell.
#[cfg(any(feature = "plain", feature = "markdown", feature = "gfm"))]
pub(crate) fn filled_cells(row: &[Vec<Piece>], field: &[usize]) -> Vec<Vec<String>> {
    row.iter()
        .enumerate()
        .map(|(index, pieces)| {
            let width = field.get(index).copied().unwrap_or(0);
            // A cell always reflows to its computed column width: the width is a layout constraint of
            // the table, not a paragraph wrap the document option can switch off.
            let text = fill(pieces, width, WrapMode::Auto);
            if text.is_empty() {
                vec![String::new()]
            } else {
                text.split('\n').map(str::to_owned).collect()
            }
        })
        .collect()
}

/// Append the body rows of a multiline table, separating rows with a blank line. A lone row still
/// gets a trailing blank to keep it visually distinct from the closing rule.
#[cfg(any(feature = "plain", feature = "markdown", feature = "gfm"))]
pub(crate) fn extend_multiline_body(
    lines: &mut Vec<String>,
    body: &[Vec<Vec<Piece>>],
    field: &[usize],
    aligns: &[&Alignment],
) {
    let count = body.len();
    for (index, row) in body.iter().enumerate() {
        lines.extend(lay_row(&filled_cells(row, field), field, aligns));
        let last = index + 1 == count;
        if !last || count == 1 {
            lines.push(String::new());
        }
    }
}

/// Indent every non-empty line by `indent` columns, leaving blank lines empty.
#[cfg(any(feature = "plain", feature = "markdown", feature = "gfm"))]
pub(crate) fn indent_lines(lines: &[String], indent: usize) -> String {
    let prefix = " ".repeat(indent);
    lines
        .iter()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("{prefix}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The natural (unwrapped) width and longest-word width of a sequence of inline pieces.
#[cfg(any(feature = "plain", feature = "markdown", feature = "gfm"))]
pub(crate) fn measure_pieces(pieces: &[Piece]) -> (usize, usize) {
    let mut natural = 0usize;
    let mut line = 0usize;
    let mut word = 0usize;
    let mut minword = 0usize;
    for piece in pieces {
        match piece {
            Piece::Text(text) => {
                let width = display_width(text);
                line += width;
                word += width;
            }
            Piece::Space | Piece::Soft => {
                line += 1;
                minword = minword.max(word);
                word = 0;
            }
            Piece::Hard => {
                natural = natural.max(line);
                minword = minword.max(word);
                line = 0;
                word = 0;
            }
        }
    }
    (natural.max(line), minword.max(word))
}

/// Whether any piece carries non-empty text.
#[cfg(any(feature = "plain", feature = "markdown", feature = "gfm"))]
pub(crate) fn pieces_nonempty(pieces: &[Piece]) -> bool {
    pieces
        .iter()
        .any(|piece| matches!(piece, Piece::Text(text) if !text.is_empty()))
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
        assert_eq!(char_width('\u{0301}'), 0);
        assert_eq!(char_width('\u{200B}'), 0);
        assert_eq!(char_width('\u{4E00}'), 2);
        assert_eq!(char_width('\u{FF21}'), 2);
        assert_eq!(char_width('\u{1F600}'), 2);
    }

    #[test]
    fn display_width_sums_characters() {
        assert_eq!(display_width(""), 0);
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("a\u{4E00}b"), 4);
        assert_eq!(display_width("e\u{0301}"), 1);
    }

    #[test]
    fn escape_xml_handles_metacharacters() {
        assert_eq!(escape_xml("a<b>&c", false), "a&lt;b&gt;&amp;c");
        assert_eq!(escape_xml("\"q\"", false), "\"q\"");
        assert_eq!(escape_xml("\"q\"", true), "&quot;q&quot;");
        assert_eq!(escape_attr("<\"&>"), "&lt;&quot;&amp;&gt;");
    }

    #[test]
    fn percent_escaped_uri_validates_escapes_and_charset() {
        assert!(is_percent_escaped_uri("abc", false));
        assert!(is_percent_escaped_uri("a/b?c#d", false));
        assert!(is_percent_escaped_uri("a%20b", false));
        assert!(!is_percent_escaped_uri("a%2", false));
        assert!(!is_percent_escaped_uri("a%zz", false));
        assert!(!is_percent_escaped_uri("a b", false));
        assert!(!is_percent_escaped_uri("café", false));
        assert!(is_percent_escaped_uri("café", true));
    }

    #[test]
    fn uri_scheme_recognition() {
        assert!(!is_uri_scheme(""));
        assert!(is_uri_scheme("http"));
        assert!(is_uri_scheme("x+y-z.w"));
        assert!(!is_uri_scheme("1abc"));
        assert!(!is_uri_scheme("ab cd"));
    }

    #[test]
    fn numeral_renders_every_style() {
        assert_eq!(numeral(5, &ListNumberStyle::Decimal), "5");
        assert_eq!(numeral(5, &ListNumberStyle::DefaultStyle), "5");
        assert_eq!(numeral(5, &ListNumberStyle::Example), "5");
        assert_eq!(numeral(1, &ListNumberStyle::LowerAlpha), "a");
        assert_eq!(numeral(27, &ListNumberStyle::LowerAlpha), "aa");
        assert_eq!(numeral(1, &ListNumberStyle::UpperAlpha), "A");
        assert_eq!(numeral(28, &ListNumberStyle::UpperAlpha), "AB");
        assert_eq!(numeral(4, &ListNumberStyle::LowerRoman), "iv");
        assert_eq!(numeral(9, &ListNumberStyle::LowerRoman), "ix");
        assert_eq!(numeral(2024, &ListNumberStyle::UpperRoman), "MMXXIV");
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
        assert_eq!(wrap_delim("3", &ListNumberDelim::Period), "3.");
        assert_eq!(wrap_delim("3", &ListNumberDelim::DefaultDelim), "3.");
        assert_eq!(wrap_delim("3", &ListNumberDelim::OneParen), "3)");
        assert_eq!(wrap_delim("3", &ListNumberDelim::TwoParens), "(3)");
        assert_eq!(
            ordered_marker(2, &ListNumberStyle::LowerRoman, &ListNumberDelim::OneParen),
            "ii)"
        );
    }

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

    #[test]
    fn known_attribute_recognition() {
        assert!(is_known_attribute("href"));
        assert!(is_known_attribute("colspan"));
        assert!(is_known_attribute("data-x"));
        assert!(is_known_attribute("aria-label"));
        assert!(is_known_attribute("epub:type"));
        assert!(is_known_attribute("xml:lang"));
        assert!(!is_known_attribute("wibble"));
    }

    #[test]
    fn render_html_attr_orders_and_prefixes() {
        let attr = Attr {
            id: "x<".into(),
            classes: vec!["a".into(), "b".into()],
            attributes: vec![
                ("href".into(), "/p?q=1&r=2".into()),
                ("wibble".into(), "v".into()),
            ],
        };
        assert_eq!(
            render_html_attr(&attr),
            " id=\"x&lt;\" class=\"a b\" href=\"/p?q=1&amp;r=2\" data-wibble=\"v\""
        );
        assert_eq!(render_html_attr(&Attr::default()), "");
    }

    #[test]
    fn attribute_value_lookup() {
        let attr = Attr {
            attributes: vec![("k".into(), "v".into())],
            ..Attr::default()
        };
        assert_eq!(attribute_value(&attr, "k"), Some("v"));
        assert_eq!(attribute_value(&attr, "missing"), None);
    }

    #[test]
    fn fill_wraps_at_column_boundary() {
        let pieces = vec![
            Piece::Text("hello".into()),
            Piece::Space,
            Piece::Text("world".into()),
        ];
        assert_eq!(fill(&pieces, 72, WrapMode::Auto), "hello world");
        assert_eq!(fill(&pieces, 8, WrapMode::Auto), "hello\nworld");
    }

    #[test]
    fn fill_collapses_spaces_and_keeps_runs_together() {
        let pieces = vec![
            Piece::Space,
            Piece::Text("ab".into()),
            Piece::Text("cd".into()),
            Piece::Space,
            Piece::Space,
            Piece::Text("ef".into()),
            Piece::Space,
        ];
        assert_eq!(fill(&pieces, 72, WrapMode::Auto), "abcd ef");
    }

    #[test]
    fn fill_honors_hard_break() {
        let pieces = vec![
            Piece::Text("a".into()),
            Piece::Hard,
            Piece::Text("b".into()),
        ];
        assert_eq!(fill(&pieces, 72, WrapMode::Auto), "a\nb");
    }

    #[test]
    fn fill_offset_shifts_first_line_wrap() {
        let pieces = vec![
            Piece::Text("aa".into()),
            Piece::Space,
            Piece::Text("bb".into()),
        ];
        assert_eq!(fill_offset(&pieces, 6, 3, WrapMode::Auto), "aa\nbb");
        assert_eq!(fill_offset(&pieces, 8, 3, WrapMode::Auto), "aa bb");
    }

    #[test]
    fn fill_none_never_wraps_and_softens_breaks() {
        let pieces = vec![
            Piece::Text("hello".into()),
            Piece::Soft,
            Piece::Text("world".into()),
            Piece::Space,
            Piece::Text("again".into()),
        ];
        // No wrapping despite the tiny width, and the soft break is just a space.
        assert_eq!(fill(&pieces, 4, WrapMode::None), "hello world again");
    }

    #[test]
    fn fill_preserve_keeps_soft_breaks_but_does_not_reflow() {
        let pieces = vec![
            Piece::Text("hello".into()),
            Piece::Soft,
            Piece::Text("world".into()),
            Piece::Space,
            Piece::Text("again".into()),
        ];
        // The soft break stays a line break; the long second line is not reflowed.
        assert_eq!(fill(&pieces, 4, WrapMode::Preserve), "hello\nworld again");
    }

    #[test]
    fn fill_auto_treats_soft_break_as_a_reflow_point() {
        let pieces = vec![
            Piece::Text("hello".into()),
            Piece::Soft,
            Piece::Text("world".into()),
        ];
        // A soft break reflows exactly like a space under Auto.
        assert_eq!(fill(&pieces, 72, WrapMode::Auto), "hello world");
        assert_eq!(fill(&pieces, 8, WrapMode::Auto), "hello\nworld");
    }

    #[test]
    fn indent_block_applies_hanging_prefixes() {
        assert_eq!(indent_block("a\nb\n\nc", "- ", "  "), "- a\n  b\n\n  c");
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

    #[test]
    fn append_notes_sections() {
        assert_eq!(append_notes("body\n".to_owned(), &[]), "body");
        assert_eq!(
            append_notes("body".to_owned(), &["[1] note".to_owned()]),
            "body\n\n[1] note"
        );
        assert_eq!(
            append_notes(String::new(), &["[1] note".to_owned()]),
            "[1] note"
        );
    }

    #[cfg(feature = "html")]
    #[test]
    fn parse_dimension_classifies_pixels_percent_and_length() {
        assert_eq!(parse_dimension("200"), Some(Dimension::Pixels(200)));
        assert_eq!(parse_dimension("200px"), Some(Dimension::Pixels(200)));
        // A fractional pixel value truncates toward zero.
        assert_eq!(parse_dimension("200.99px"), Some(Dimension::Pixels(200)));
        assert_eq!(parse_dimension("0.6"), Some(Dimension::Pixels(0)));
        assert_eq!(parse_dimension("0"), Some(Dimension::Pixels(0)));
        assert_eq!(parse_dimension("50%"), Some(Dimension::Percent(50.0)));
        assert_eq!(parse_dimension("12.5%"), Some(Dimension::Percent(12.5)));
        assert_eq!(parse_dimension("5cm"), Some(Dimension::Length(5.0, "cm")));
        assert_eq!(parse_dimension("72pt"), Some(Dimension::Length(72.0, "pt")));
        assert_eq!(parse_dimension("3em"), Some(Dimension::Length(3.0, "em")));
        // `inch` normalizes to the `in` unit.
        assert_eq!(
            parse_dimension("10inch"),
            Some(Dimension::Length(10.0, "in"))
        );
        assert_eq!(parse_dimension("2in"), Some(Dimension::Length(2.0, "in")));
    }

    #[cfg(feature = "html")]
    #[test]
    fn parse_dimension_rejects_unrecognized_shapes() {
        // Unknown units, signs, surrounding space, malformed numbers, and uppercase units all yield
        // no dimension, so the attribute is dropped.
        assert_eq!(parse_dimension("4ex"), None);
        assert_eq!(parse_dimension("50vw"), None);
        assert_eq!(parse_dimension("3rem"), None);
        assert_eq!(parse_dimension("-200"), None);
        assert_eq!(parse_dimension("-50%"), None);
        assert_eq!(parse_dimension("+5cm"), None);
        assert_eq!(parse_dimension(" 5cm "), None);
        assert_eq!(parse_dimension("5 cm"), None);
        assert_eq!(parse_dimension(".5cm"), None);
        assert_eq!(parse_dimension("5.cm"), None);
        assert_eq!(parse_dimension("5CM"), None);
        assert_eq!(parse_dimension("5PX"), None);
        assert_eq!(parse_dimension(""), None);
    }

    #[cfg(feature = "html")]
    #[test]
    fn format_percent_keeps_at_least_one_decimal() {
        assert_eq!(format_percent_dimension(50.0), "50.0%");
        assert_eq!(format_percent_dimension(100.0), "100.0%");
        assert_eq!(format_percent_dimension(0.0), "0.0%");
        assert_eq!(format_percent_dimension(12.5), "12.5%");
        assert_eq!(format_percent_dimension(33.333_333), "33.333333%");
    }

    #[cfg(feature = "html")]
    #[test]
    fn format_length_rounds_and_strips_trailing_zeros() {
        assert_eq!(format_length_dimension(5.0, "cm"), "5cm");
        assert_eq!(format_length_dimension(100.0, "pt"), "100pt");
        assert_eq!(format_length_dimension(2.54, "in"), "2.54in");
        assert_eq!(format_length_dimension(12.3, "cm"), "12.3cm");
        // Rounded to five fractional digits.
        assert_eq!(format_length_dimension(12.345_678_9, "cm"), "12.34568cm");
        assert_eq!(format_length_dimension(1.123_456, "cm"), "1.12346cm");
    }

    #[cfg(feature = "html")]
    #[test]
    fn normalize_image_attr_splits_pixel_and_style_dimensions() {
        let pixels = Attr {
            attributes: vec![("width".into(), "200px".into())],
            ..Attr::default()
        };
        assert_eq!(
            normalize_image_attr(&pixels).attributes,
            vec![("width".to_owned(), "200".to_owned())]
        );
        let percent = Attr {
            attributes: vec![("width".into(), "50%".into())],
            ..Attr::default()
        };
        assert_eq!(
            normalize_image_attr(&percent).attributes,
            vec![("style".to_owned(), "width:50.0%".to_owned())]
        );
    }

    #[cfg(feature = "html")]
    #[test]
    fn normalize_image_attr_orders_style_then_rest_then_pixels() {
        // Source order: a regular pair, a percentage width, a pixel height, another regular pair.
        // The combined style leads, the regular pairs keep their order, the pixel attribute trails.
        let attr = Attr {
            attributes: vec![
                ("data-a".into(), "1".into()),
                ("width".into(), "50%".into()),
                ("height".into(), "200px".into()),
                ("loading".into(), "lazy".into()),
            ],
            ..Attr::default()
        };
        assert_eq!(
            normalize_image_attr(&attr).attributes,
            vec![
                ("style".to_owned(), "width:50.0%".to_owned()),
                ("data-a".to_owned(), "1".to_owned()),
                ("loading".to_owned(), "lazy".to_owned()),
                ("height".to_owned(), "200".to_owned()),
            ]
        );
    }

    #[cfg(feature = "html")]
    #[test]
    fn normalize_image_attr_emits_width_before_height() {
        // Height precedes width in the source, yet width renders first.
        let attr = Attr {
            attributes: vec![
                ("height".into(), "100".into()),
                ("width".into(), "200".into()),
            ],
            ..Attr::default()
        };
        assert_eq!(
            normalize_image_attr(&attr).attributes,
            vec![
                ("width".to_owned(), "200".to_owned()),
                ("height".to_owned(), "100".to_owned()),
            ]
        );
        let both_style = Attr {
            attributes: vec![
                ("height".into(), "5cm".into()),
                ("width".into(), "50%".into()),
            ],
            ..Attr::default()
        };
        assert_eq!(
            normalize_image_attr(&both_style).attributes,
            vec![("style".to_owned(), "width:50.0%;height:5cm".to_owned())]
        );
    }

    #[cfg(feature = "html")]
    #[test]
    fn normalize_image_attr_appends_dimensions_to_existing_style() {
        let attr = Attr {
            attributes: vec![
                ("style".into(), "color:red".into()),
                ("width".into(), "50%".into()),
            ],
            ..Attr::default()
        };
        assert_eq!(
            normalize_image_attr(&attr).attributes,
            vec![("style".to_owned(), "color:red;width:50.0%".to_owned())]
        );
        // A source style with no dimensions still moves ahead of the remaining pairs.
        let style_only = Attr {
            attributes: vec![
                ("data-a".into(), "1".into()),
                ("style".into(), "color:red".into()),
                ("data-b".into(), "2".into()),
            ],
            ..Attr::default()
        };
        assert_eq!(
            normalize_image_attr(&style_only).attributes,
            vec![
                ("style".to_owned(), "color:red".to_owned()),
                ("data-a".to_owned(), "1".to_owned()),
                ("data-b".to_owned(), "2".to_owned()),
            ]
        );
    }

    #[cfg(feature = "html")]
    #[test]
    fn normalize_image_attr_drops_unrecognized_dimensions_and_keeps_id_class() {
        let attr = Attr {
            id: "x".into(),
            classes: vec!["c".into()],
            attributes: vec![
                ("width".into(), "4ex".into()),
                ("height".into(), "100".into()),
            ],
        };
        let normalized = normalize_image_attr(&attr);
        assert_eq!(normalized.id, "x");
        assert_eq!(normalized.classes, vec!["c".to_owned()]);
        // The unparsable width is dropped; the pixel height survives.
        assert_eq!(
            normalized.attributes,
            vec![("height".to_owned(), "100".to_owned())]
        );
    }
}
