//! Free helpers for the ODT reader: anchor interning, block flushing, text and inline shaping,
//! style-property parsing, length decoding, and table grid squaring.

use std::collections::BTreeMap;

use carta_ast::{Alignment, Attr, Block, Cell, Inline, ListNumberDelim, ListNumberStyle, Row};

use crate::heading_ids::IdRegistry;
use crate::xml::{Element, Node, local_name};

use super::{TextProps, Vertical};

/// The identifier an anchor name resolves to, assigned once on first sighting and reused for every
/// later use of the same name so the anchor and any reference to it share one target. `seed` is the
/// base the fresh identifier is disambiguated from: a fixed label for a name that is dropped, or the
/// name itself where it is kept.
pub(super) fn intern_anchor(
    map: &mut BTreeMap<String, String>,
    ids: &mut IdRegistry,
    name: &str,
    seed: &str,
) -> String {
    if let Some(existing) = map.get(name) {
        return existing.clone();
    }
    let assigned = ids.assign_with_separator(seed.to_owned(), '-');
    map.insert(name.to_owned(), assigned.clone());
    assigned
}

pub(super) fn flush_quote(out: &mut Vec<Block>, quote: &mut Vec<Block>) {
    if !quote.is_empty() {
        out.push(Block::BlockQuote(std::mem::take(quote)));
    }
}

pub(super) fn flush_code(out: &mut Vec<Block>, code: &mut Vec<String>) {
    if !code.is_empty() {
        out.push(Block::CodeBlock(
            Box::default(),
            std::mem::take(code).join("\n").into(),
        ));
    }
}

/// Collapses a single-paragraph block sequence to a bare `Plain`, the compact shape a list item or
/// table cell carries when it holds nothing but one paragraph.
pub(super) fn compact(mut blocks: Vec<Block>) -> Vec<Block> {
    if blocks.len() == 1
        && matches!(blocks.first(), Some(Block::Para(_)))
        && let Some(Block::Para(inlines)) = blocks.pop()
    {
        blocks.push(Block::Plain(inlines));
    }
    blocks
}

/// Splits a run of character data into `Str` words separated by whitespace inlines: a whitespace run
/// containing a line ending becomes a soft break, any other whitespace run a single space. Whitespace
/// at the edges is kept, since a run may abut formatting on either side.
pub(super) fn push_text(out: &mut Vec<Inline>, text: &str) {
    let mut word = String::new();
    let mut chars = text.chars().peekable();
    while let Some(&ch) = chars.peek() {
        // Only ASCII whitespace collapses; NBSP, em space, separators, etc. are content.
        if ch.is_ascii_whitespace() {
            if !word.is_empty() {
                out.push(Inline::Str(std::mem::take(&mut word).into()));
            }
            let mut line_ending = false;
            while let Some(&ws) = chars.peek() {
                if ws.is_ascii_whitespace() {
                    line_ending = line_ending || ws == '\n' || ws == '\r';
                    chars.next();
                } else {
                    break;
                }
            }
            out.push(if line_ending {
                Inline::SoftBreak
            } else {
                Inline::Space
            });
        } else {
            word.push(ch);
            chars.next();
        }
    }
    if !word.is_empty() {
        out.push(Inline::Str(word.into()));
    }
}

/// Fuses runs of adjacent text into one, so a marker that carries no content of its own (a bookmark
/// end, a reference-mark point, an unstyled span) leaves no seam between the words around it.
pub(super) fn coalesce_text(inlines: &mut Vec<Inline>) {
    // The common run has no adjacent text pieces to fuse; skip the rebuild and its allocation then.
    if !inlines
        .windows(2)
        .any(|pair| matches!(pair, [Inline::Str(_), Inline::Str(_)]))
    {
        return;
    }
    let mut merged: Vec<Inline> = Vec::with_capacity(inlines.len());
    for inline in inlines.drain(..) {
        if let Inline::Str(text) = &inline
            && let Some(Inline::Str(previous)) = merged.last_mut()
        {
            previous.push_str(text);
            continue;
        }
        merged.push(inline);
    }
    *inlines = merged;
}

/// Wraps inline content in the formatting a character style declares, nested outermost-first:
/// superscript or subscript, then emphasis, then strong, then strikeout.
pub(super) fn apply_wrappers(props: TextProps, inner: Vec<Inline>) -> Vec<Inline> {
    let mut inlines = inner;
    if props.strike {
        inlines = vec![Inline::Strikeout(inlines)];
    }
    if props.strong {
        inlines = vec![Inline::Strong(inlines)];
    }
    if props.emph {
        inlines = vec![Inline::Emph(inlines)];
    }
    match props.vertical {
        Vertical::Super => vec![Inline::Superscript(inlines)],
        Vertical::Sub => vec![Inline::Subscript(inlines)],
        Vertical::Baseline => inlines,
    }
}

/// Flattens inline content to its plain text, the form a code span or code block carries and the
/// basis for a heading's slug. Spaces and line breaks render as their literal characters.
pub(super) fn inlines_to_plain(inlines: &[Inline]) -> String {
    let mut out = String::new();
    collect_plain(inlines, &mut out);
    out
}

pub(super) fn empty_span(id: String) -> Inline {
    Inline::Span(
        Box::new(Attr {
            id: id.into(),
            classes: Vec::new(),
            attributes: Vec::new(),
        }),
        Vec::new(),
    )
}

#[allow(clippy::match_same_arms)]
fn collect_plain(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Str(text) => out.push_str(text),
            Inline::Space => out.push(' '),
            Inline::SoftBreak | Inline::LineBreak => out.push('\n'),
            Inline::Code(_, text) => out.push_str(text),
            Inline::Emph(children)
            | Inline::Strong(children)
            | Inline::Strikeout(children)
            | Inline::Superscript(children)
            | Inline::Subscript(children)
            | Inline::Underline(children)
            | Inline::SmallCaps(children)
            | Inline::Span(_, children)
            | Inline::Link(_, children, _) => collect_plain(children, out),
            Inline::Image(_, alt, _) => collect_plain(alt, out),
            _ => {}
        }
    }
}

pub(super) fn read_text_props(decoded_name: &str, style: &Element) -> TextProps {
    let mut props = TextProps {
        code: decoded_name == "Source Text",
        ..TextProps::default()
    };
    let Some(text_props) = style.child("text-properties") else {
        return props;
    };
    if let Some(weight) = text_props.attr("font-weight") {
        props.strong = is_bold(weight);
    }
    if text_props.attr("font-style").is_some_and(is_italic) {
        props.emph = true;
    }
    if text_props
        .attr("text-underline-style")
        .is_some_and(|value| value != "none")
    {
        props.emph = true;
    }
    if text_props
        .attr("text-line-through-style")
        .is_some_and(|value| value != "none")
    {
        props.strike = true;
    }
    if let Some(position) = text_props.attr("text-position") {
        props.vertical = parse_position(position);
    }
    props
}

fn is_bold(weight: &str) -> bool {
    weight == "bold" || weight.parse::<u32>().is_ok_and(|value| value >= 700)
}

fn is_italic(style: &str) -> bool {
    matches!(style, "italic" | "oblique")
}

/// Reads a `style:text-position`, whose first token is `super`, `sub`, or a signed percentage that
/// raises the baseline (positive) or lowers it (negative).
fn parse_position(position: &str) -> Vertical {
    let first = position.split_whitespace().next().unwrap_or_default();
    if first.starts_with("super") {
        return Vertical::Super;
    }
    if first.starts_with("sub") {
        return Vertical::Sub;
    }
    match first.trim_end_matches('%').parse::<f64>() {
        Ok(value) if value > 0.0 => Vertical::Super,
        Ok(value) if value < 0.0 => Vertical::Sub,
        _ => Vertical::Baseline,
    }
}

pub(super) fn map_number_style(format: Option<&str>) -> ListNumberStyle {
    match format {
        Some("i") => ListNumberStyle::LowerRoman,
        Some("I") => ListNumberStyle::UpperRoman,
        Some("a") => ListNumberStyle::LowerAlpha,
        Some("A") => ListNumberStyle::UpperAlpha,
        _ => ListNumberStyle::Decimal,
    }
}

/// Maps a marker's surrounding punctuation to a delimiter: a closing parenthesis with a matching
/// opener encloses the number, a lone closing parenthesis trails it, and a period trails it.
pub(super) fn map_delim(prefix: &str, suffix: &str) -> ListNumberDelim {
    if suffix == ")" {
        if prefix == "(" {
            ListNumberDelim::TwoParens
        } else {
            ListNumberDelim::OneParen
        }
    } else if suffix == "." {
        ListNumberDelim::Period
    } else {
        ListNumberDelim::DefaultDelim
    }
}

/// Parses an absolute length such as `1cm` or `0.5in` into inches, so lengths in different units
/// compare on one scale. Relative measures (a percentage), unitless numbers, and unknown units name no
/// resolvable absolute length and yield `None`.
pub(super) fn parse_length(value: &str) -> Option<f64> {
    let value = value.trim();
    let end = value
        .char_indices()
        .find(|(_, ch)| !(ch.is_ascii_digit() || matches!(ch, '.' | '-' | '+')))
        .map_or(value.len(), |(index, _)| index);
    let magnitude = value.get(..end)?.parse::<f64>().ok()?;
    let per_inch = match value.get(end..).unwrap_or("").trim() {
        "in" => 1.0,
        "cm" => 2.54,
        "mm" => 25.4,
        "pt" => 72.0,
        "pc" => 6.0,
        "px" => 96.0,
        _ => return None,
    };
    Some(magnitude / per_inch)
}

/// Decodes the `_HH.._` hex escapes an ODF style name uses for characters (notably `_20_` for a
/// space), leaving every other character untouched.
pub(super) fn decode_style_name(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::with_capacity(name.len());
    let mut index = 0;
    while let Some(&ch) = chars.get(index) {
        if ch != '_' {
            out.push(ch);
            index += 1;
            continue;
        }
        let mut end = index + 1;
        while chars.get(end).is_some_and(char::is_ascii_hexdigit) {
            end += 1;
        }
        if end > index + 1 && end <= index + 7 && chars.get(end) == Some(&'_') {
            let hex: String = chars.get(index + 1..end).unwrap_or(&[]).iter().collect();
            if let Some(decoded) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                out.push(decoded);
                index = end + 1;
                continue;
            }
        }
        out.push('_');
        index += 1;
    }
    out
}

/// The text box a figure paragraph wraps, or `None` if the paragraph is ordinary prose. A figure
/// paragraph holds nothing but a single frame that in turn holds a text box, the shape a captioned
/// image takes; any sibling text or a second element keeps the frame inline instead.
pub(super) fn figure_paragraph(paragraph: &Element) -> Option<&Element> {
    let mut frame = None;
    for node in &paragraph.children {
        match node {
            Node::Text(text) if text.trim().is_empty() => {}
            Node::Element(element) if local_name(&element.name) == "frame" => {
                if frame.is_some() {
                    return None;
                }
                frame = Some(element);
            }
            _ => return None,
        }
    }
    frame?.child("text-box")
}

/// Whether a qualified element name belongs to the drawing namespace, the shapes ODF uses for
/// frames, text boxes, and other floating objects (its conventional prefix is `draw`). A drawing
/// shape anchored at block level is floating layout rather than body flow.
pub(super) fn is_drawing_shape(name: &str) -> bool {
    matches!(name.split_once(':'), Some(("draw", _)))
}

/// The archive path of a formula sub-object's MathML part: the referenced object directory joined
/// with its `content.xml`, with any leading `./` and trailing slash trimmed.
pub(super) fn formula_part_path(href: &str) -> String {
    let base = href.trim_start_matches("./").trim_end_matches('/');
    format!("{base}/content.xml")
}

/// The widest row's column count, summing each cell's column span with saturating arithmetic so a
/// cell declaring an outsized span cannot overflow the running total.
pub(super) fn row_width(rows: &[Row]) -> i32 {
    rows.iter().map(cells_width).max().unwrap_or(0)
}

/// A row's occupied column count: the sum of its cells' column spans, saturating so a cell declaring
/// an outsized span cannot overflow the running total.
fn cells_width(row: &Row) -> i32 {
    row.cells
        .iter()
        .fold(0i32, |acc, cell| acc.saturating_add(cell.col_span.max(1)))
}

/// Squares each row off to the grid width by appending empty single-column cells, so every row spans
/// the same number of columns, while leaving columns already occupied by a row-spanning cell
/// overhanging from an earlier row unfilled. A row whose cells plus inherited overhang already reach
/// the width is left untouched.
pub(super) fn square_rows(rows: &mut [Row], columns: i32) {
    let width = usize::try_from(columns).unwrap_or(0);
    // `covered[c]`: how many further rows column `c` stays covered by a row span from above.
    let mut covered = vec![0i32; width];
    for row in rows {
        let overhang =
            i32::try_from(covered.iter().filter(|count| **count > 0).count()).unwrap_or(i32::MAX);
        // Walk real cells across the grid, skipping covered columns, to find this row's new spans.
        let mut new_cover = vec![0i32; width];
        let mut column = 0usize;
        for cell in &row.cells {
            while covered.get(column).is_some_and(|count| *count > 0) {
                column += 1;
            }
            let span = usize::try_from(cell.col_span.max(1))
                .unwrap_or(1)
                .min(width.saturating_sub(column));
            if cell.row_span > 1 {
                for offset in 0..span {
                    if let Some(slot) = new_cover.get_mut(column + offset) {
                        *slot = cell.row_span - 1;
                    }
                }
            }
            column = column.saturating_add(usize::try_from(cell.col_span.max(1)).unwrap_or(1));
        }
        for _ in cells_width(row)..columns.saturating_sub(overhang) {
            row.cells.push(empty_cell());
        }
        for (slot, added) in covered.iter_mut().zip(new_cover) {
            *slot = if added > 0 { added } else { (*slot - 1).max(0) };
        }
    }
}

fn empty_cell() -> Cell {
    Cell {
        attr: Attr::default(),
        align: Alignment::AlignDefault,
        row_span: 1,
        col_span: 1,
        content: Vec::new(),
    }
}
