//! `DokuWiki` writer: renders the document model to `DokuWiki` markup.
//!
//! Inline content is emitted on a single line — a soft break becomes a space and block structure is
//! conveyed through `DokuWiki`'s line-oriented markup. Top-level blocks are separated by a blank
//! line. Output carries no trailing newline; the caller appends one. This format has no public
//! specification, so its rules are stated directly here.

use std::fmt::Write as _;

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, Document, Format, Inline, MathType, Row, Table, Target,
    to_plain_text,
};
use carta_core::{Result, Writer, WriterOptions};

use crate::common::{
    self, GridSlot, RawTrim, RowSpanGrid, attribute_value, quote_marks, split_length_unit,
};

/// Columns each level of list nesting adds, and the base indent of a top-level list line.
const LIST_INDENT: usize = 2;

/// Renders a document to `DokuWiki` markup.
#[derive(Debug, Default, Clone, Copy)]
pub struct DokuwikiWriter;

impl Writer for DokuwikiWriter {
    fn write(&self, document: &Document, _options: &WriterOptions) -> Result<String> {
        let body = render_blocks(&document.blocks, "\n\n");
        Ok(body)
    }
}

/// Render a block sequence, dropping blocks that produce nothing and joining the rest with
/// `separator`.
fn render_blocks(blocks: &[Block], separator: &str) -> String {
    blocks
        .iter()
        .map(block)
        .filter(|rendered| !rendered.is_empty())
        .collect::<Vec<_>>()
        .join(separator)
}

fn block(block: &Block) -> String {
    match block {
        Block::Plain(inlines) | Block::Para(inlines) => inlines_to_markup(inlines),
        Block::LineBlock(lines) => line_block(lines),
        Block::CodeBlock(attr, text) => code_block(attr, text),
        Block::RawBlock(format, text) => raw_passthrough(format, text),
        Block::BlockQuote(blocks) => block_quote(blocks),
        Block::BulletList(items) => bullet_list(items, LIST_INDENT),
        Block::OrderedList(_, items) => ordered_list(items, LIST_INDENT),
        Block::DefinitionList(items) => definition_list(items),
        Block::Header(level, _, inlines) => header(*level, inlines),
        Block::HorizontalRule => "\n----".to_owned(),
        Block::Table(table) => render_table(table),
        Block::Figure(_, caption, blocks) => figure(caption, blocks),
        Block::Div(_, blocks) => div(blocks),
    }
}

/// A heading: a run of `=` whose length decreases as the level deepens (level 1 is the widest), with
/// the heading text — markup stripped to plain text — set off by single spaces.
fn header(level: i32, inlines: &[Inline]) -> String {
    let depth = level.clamp(1, 6);
    let equals = "=".repeat((7 - depth).unsigned_abs() as usize);
    let text = bare_inlines(inlines);
    format!("{equals} {text} {equals}")
}

/// A line block: its lines on consecutive output lines, each but the last ending in the forced-break
/// marker `\\`.
fn line_block(lines: &[Vec<Inline>]) -> String {
    lines
        .iter()
        .map(|line| inlines_to_markup(line))
        .collect::<Vec<_>>()
        .join("\\\\\n")
}

/// A fenced code block: the verbatim payload between `<code>` tags, the first class naming the
/// source language when present.
fn code_block(attr: &Attr, text: &str) -> String {
    match attr.classes.first() {
        Some(language) if !language.is_empty() => format!("<code {language}>\n{text}\n</code>"),
        _ => format!("<code>\n{text}\n</code>"),
    }
}

/// A block quote: every non-empty line of the inner content prefixed with `> `; blank separator
/// lines are left bare.
fn block_quote(blocks: &[Block]) -> String {
    let body = render_blocks(blocks, "\n\n");
    body.lines()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else if line.starts_with('>') {
                format!(">{line}")
            } else {
                format!("> {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A division renders its inner blocks transparently. It is set off from what follows by a blank
/// line only when its content ends in a block that itself stands on its own paragraph — an inline
/// block (`Plain`) or a heading carries no such trailing blank. An empty division renders to
/// nothing.
fn div(blocks: &[Block]) -> String {
    let body = render_blocks(blocks, "\n\n");
    if body.is_empty() {
        String::new()
    } else if blocks.last().is_some_and(closes_with_blank) {
        format!("{body}\n")
    } else {
        body
    }
}

/// Whether a block, ending a division, leaves a trailing blank line after it. Inline-level content
/// (`Plain`) and headings do not; other block constructs do. Transparent wrappers defer to their
/// own last child.
fn closes_with_blank(block: &Block) -> bool {
    match block {
        Block::Plain(_) | Block::Header(..) => false,
        Block::Div(_, inner) | Block::BlockQuote(inner) => {
            inner.last().is_some_and(closes_with_blank)
        }
        _ => true,
    }
}

/// A figure: its body blocks followed by its caption blocks, then a blank line that sets the figure
/// off from what follows.
fn figure(caption: &Caption, blocks: &[Block]) -> String {
    let body = render_blocks(blocks, "\n\n");
    let cap = render_blocks(&caption.long, "\n\n");
    let content = match (body.is_empty(), cap.is_empty()) {
        (false, false) => format!("{body}\n{cap}"),
        (false, true) => body,
        (true, false) => cap,
        (true, true) => String::new(),
    };
    if content.is_empty() {
        String::new()
    } else {
        format!("{content}\n")
    }
}

fn bullet_list(items: &[Vec<Block>], indent: usize) -> String {
    render_list(items, indent, '*')
}

fn ordered_list(items: &[Vec<Block>], indent: usize) -> String {
    render_list(items, indent, '-')
}

/// Render a bullet (`*`) or ordered (`-`) list. Each item line opens with `indent` spaces, the
/// marker, and a space. An item renders compactly when it is one leading text block followed only by
/// sublists; any other shape is wrapped in a `<WRAP>` block.
fn render_list(items: &[Vec<Block>], indent: usize, marker: char) -> String {
    let prefix = format!("{}{marker} ", " ".repeat(indent));
    items
        .iter()
        .map(|item| list_item(item, indent, &prefix))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render a single list item. An item renders compactly when it is empty, a lone text block, a text
/// block followed only by sublists, or a text block followed by a single code block; any other shape
/// is wrapped in a `<WRAP>` block. `indent` is this item's marker indent; sublists sit one level
/// deeper.
fn list_item(item: &[Block], indent: usize, prefix: &str) -> String {
    if let Some(simple) = simple_item(item, indent, prefix) {
        return simple;
    }
    let body = wrap_body(item.iter(), indent + LIST_INDENT);
    format!("{prefix}<WRAP>\n{body}\n</WRAP>")
}

fn simple_item(item: &[Block], indent: usize, prefix: &str) -> Option<String> {
    let sub_indent = indent + LIST_INDENT;
    match item {
        [] => Some(prefix.trim_end().to_owned()),
        [Block::Plain(inlines) | Block::Para(inlines)] => {
            Some(format!("{prefix}{}", inlines_to_markup(inlines)))
        }
        [
            Block::Plain(inlines) | Block::Para(inlines),
            Block::CodeBlock(attr, text),
        ] => Some(format!(
            "{prefix}{}{}\n",
            inlines_to_markup(inlines),
            code_block(attr, text)
        )),
        [Block::Plain(inlines) | Block::Para(inlines), rest @ ..]
            if rest.iter().all(is_sublist) =>
        {
            let mut out = format!("{prefix}{}", inlines_to_markup(inlines));
            for sublist in rest {
                let _ = write!(out, "\n{}", sublist_markup(sublist, sub_indent));
            }
            Some(out)
        }
        _ => None,
    }
}

fn is_sublist(block: &Block) -> bool {
    matches!(block, Block::BulletList(_) | Block::OrderedList(..))
}

/// Render a sublist nested inside a list item at the given indent.
fn sublist_markup(block: &Block, indent: usize) -> String {
    match block {
        Block::BulletList(items) => bullet_list(items, indent),
        Block::OrderedList(_, items) => ordered_list(items, indent),
        other => self::block(other),
    }
}

/// The content of a `<WRAP>` block: each inner block rendered and joined by a single newline.
/// A nested list inside the wrap sits at `sub_indent`.
fn wrap_body<'a>(blocks: impl Iterator<Item = &'a Block>, sub_indent: usize) -> String {
    blocks
        .map(|block| match block {
            Block::BulletList(items) => bullet_list(items, sub_indent),
            Block::OrderedList(_, items) => ordered_list(items, sub_indent),
            // A fenced code block stands off from neighbouring content by a trailing blank line.
            Block::CodeBlock(attr, text) => format!("{}\n", code_block(attr, text)),
            other => self::block(other),
        })
        .filter(|rendered| !rendered.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// A definition list: one bullet item per entry, the term in bold followed by its definitions. A
/// single-block definition set joins inline with `; `; anything larger is wrapped.
fn definition_list(items: &[(Vec<Inline>, Vec<Vec<Block>>)]) -> String {
    items
        .iter()
        .map(|(term, definitions)| definition_entry(term, definitions))
        .collect::<Vec<_>>()
        .join("\n")
}

fn definition_entry(term: &[Inline], definitions: &[Vec<Block>]) -> String {
    let strong = format!("**{}**", inlines_to_markup(term));
    if definitions.is_empty() {
        return format!("  * {strong} ");
    }
    let inline_each: Option<Vec<String>> = definitions
        .iter()
        .map(|definition| match definition.as_slice() {
            [Block::Plain(inlines) | Block::Para(inlines)] => Some(inlines_to_markup(inlines)),
            _ => None,
        })
        .collect();
    if let Some(parts) = inline_each {
        format!("  * {strong} {}", parts.join("; "))
    } else {
        let body = wrap_body(definitions.iter().flatten(), LIST_INDENT * 2);
        format!("  * {strong} <WRAP>\n{body}\n</WRAP>")
    }
}

/// Emit a raw-passthrough payload verbatim only when its format is `DokuWiki`; any other target is
/// dropped.
fn raw_passthrough(format: &Format, text: &str) -> String {
    common::raw_passthrough(format, text, "dokuwiki", RawTrim::DropAll)
}

// --- inline rendering ---------------------------------------------------------------------------

/// Render an inline sequence to markup, collapsing each space or soft break to a single space.
fn inlines_to_markup(inlines: &[Inline]) -> String {
    inlines.iter().map(inline).collect()
}

fn inline(inline: &Inline) -> String {
    match inline {
        Inline::Str(text) => escape(text),
        Inline::Emph(inlines) => format!("//{}//", inlines_to_markup(inlines)),
        Inline::Strong(inlines) => format!("**{}**", inlines_to_markup(inlines)),
        Inline::Underline(inlines) => format!("__{}__", inlines_to_markup(inlines)),
        Inline::Strikeout(inlines) => format!("<del>{}</del>", inlines_to_markup(inlines)),
        Inline::Superscript(inlines) => format!("<sup>{}</sup>", inlines_to_markup(inlines)),
        Inline::Subscript(inlines) => format!("<sub>{}</sub>", inlines_to_markup(inlines)),
        Inline::SmallCaps(inlines) | Inline::Cite(_, inlines) | Inline::Span(_, inlines) => {
            inlines_to_markup(inlines)
        }
        Inline::Quoted(kind, inlines) => {
            let (open, close) = quote_marks(kind);
            format!("{open}{}{close}", inlines_to_markup(inlines))
        }
        Inline::Code(_, text) => format!("''%%{text}%%''"),
        Inline::Space | Inline::SoftBreak => " ".to_owned(),
        Inline::LineBreak => "\\\\\n".to_owned(),
        Inline::Math(kind, text) => match kind {
            MathType::InlineMath => format!("${text}$"),
            MathType::DisplayMath => format!("$${text}$$"),
        },
        Inline::RawInline(format, text) => raw_passthrough(format, text),
        Inline::Link(_, inlines, target) => link(inlines, target),
        Inline::Image(attr, inlines, target) => image(attr, inlines, target),
        Inline::Note(blocks) => format!("(({}\n))", render_blocks(blocks, "\n\n")),
    }
}

/// Render an inline sequence as plain text with markup stripped, as headings present it: container
/// inlines contribute their text content, quotes keep their glyphs, and text runs are escaped.
fn bare_inlines(inlines: &[Inline]) -> String {
    inlines.iter().map(bare_inline).collect()
}

fn bare_inline(inline: &Inline) -> String {
    match inline {
        Inline::Str(text) | Inline::Code(_, text) | Inline::Math(_, text) => escape(text),
        Inline::Space | Inline::SoftBreak | Inline::LineBreak => " ".to_owned(),
        Inline::Quoted(kind, inlines) => {
            let (open, close) = quote_marks(kind);
            format!("{open}{}{close}", bare_inlines(inlines))
        }
        Inline::Emph(inlines)
        | Inline::Strong(inlines)
        | Inline::Underline(inlines)
        | Inline::Strikeout(inlines)
        | Inline::Superscript(inlines)
        | Inline::Subscript(inlines)
        | Inline::SmallCaps(inlines)
        | Inline::Cite(_, inlines)
        | Inline::Span(_, inlines)
        | Inline::Link(_, inlines, _)
        | Inline::Image(_, inlines, _) => bare_inlines(inlines),
        Inline::RawInline(..) | Inline::Note(_) => String::new(),
    }
}

/// A link. When the destination equals its plain-text label exactly (and carries no space), the URL
/// stands alone; a `mailto:` link with an all-text label renders in angle brackets; otherwise the
/// `[[destination|label]]` form is used, with one leading `/` trimmed from the destination.
fn link(inlines: &[Inline], target: &Target) -> String {
    let plain = to_plain_text(inlines);
    if plain == target.url && !target.url.contains(' ') {
        return target.url.clone();
    }
    if target.url.starts_with("mailto:") && !inlines.is_empty() && is_all_text(inlines) {
        return format!("<{plain}>");
    }
    let destination = target.url.strip_prefix('/').unwrap_or(&target.url);
    format!("[[{destination}|{}]]", inlines_to_markup(inlines))
}

/// Whether an inline sequence is composed only of text and spacing, so its plain-text form carries
/// no lost markup.
fn is_all_text(inlines: &[Inline]) -> bool {
    inlines
        .iter()
        .all(|inline| matches!(inline, Inline::Str(_) | Inline::Space | Inline::SoftBreak))
}

/// An image: `{{url[?size]|caption}}`, where the caption is the title if set, else the alt text, and
/// the optional size is derived from the `width`/`height` attributes.
fn image(attr: &Attr, inlines: &[Inline], target: &Target) -> String {
    let mut head = target.url.clone();
    if let Some(size) = image_size(attr) {
        head.push('?');
        head.push_str(&size);
    }
    let caption = if target.title.is_empty() {
        inlines_to_markup(inlines)
    } else {
        target.title.clone()
    };
    if caption.is_empty() {
        format!("{{{{{head}}}}}")
    } else {
        format!("{{{{{head}|{caption}}}}}")
    }
}

/// The `width`, `widthxheight`, or `0xheight` pixel-size descriptor for an image, or `None` when
/// neither dimension yields a usable pixel value.
fn image_size(attr: &Attr) -> Option<String> {
    let width = attr_dimension(attr, "width");
    let height = attr_dimension(attr, "height");
    match (width, height) {
        (Some(w), Some(h)) => Some(format!("{w}x{h}")),
        (Some(w), None) => Some(w.to_string()),
        (None, Some(h)) => Some(format!("0x{h}")),
        (None, None) => None,
    }
}

/// Resolve a dimension attribute to a non-negative pixel count, converting common CSS units.
fn attr_dimension(attr: &Attr, name: &str) -> Option<u64> {
    dimension_pixels(attribute_value(attr, name)?)
}

/// Convert a CSS length to whole pixels at 96 dpi, truncating toward zero. Recognized units are
/// `px`, `in`, `cm`, `mm`, `pc`, `pt`, and `em`; a bare number is taken as pixels. A value that is
/// negative, non-finite, unparseable, or in an unsupported unit (e.g. `%`, `ex`) yields `None`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn dimension_pixels(raw: &str) -> Option<u64> {
    let (number, unit) = split_length_unit(raw.trim());
    let value: f64 = number.parse().ok()?;
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    let pixels = match unit {
        "" | "px" => value,
        "in" => value * 96.0,
        "cm" => value * 96.0 / 2.54,
        "mm" => value * 96.0 / 25.4,
        "pc" => value * 16.0,
        "pt" => value * 96.0 / 72.0,
        "em" => value * 16.5,
        _ => return None,
    };
    Some(pixels.trunc() as u64)
}

/// The two-character emphasis markers escaped in literal text, the same set the inline arms emit.
const EMPHASIS_MARKERS: [&str; 3] = ["//", "**", "__"];

/// Escape a text run so its emphasis-significant pairs (`//`, `**`, `__`) do not start markup: each
/// such pair is wrapped in the `%%…%%` no-wiki span.
fn escape(text: &str) -> String {
    if !text.contains(['/', '*', '_']) {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(ch) = rest.chars().next() {
        if let Some(marker) = EMPHASIS_MARKERS
            .iter()
            .find(|marker| rest.starts_with(**marker))
        {
            let _ = write!(out, "%%{marker}%%");
            rest = rest.get(marker.len()..).unwrap_or("");
        } else {
            out.push(ch);
            rest = rest.get(ch.len_utf8()..).unwrap_or("");
        }
    }
    out
}

// --- tables -------------------------------------------------------------------------------------

/// Render a table: an optional caption paragraph, then header rows marked with `^` and body rows
/// marked with `|`, every cell padded to its column width per the column's alignment.
fn render_table(table: &Table) -> String {
    let aligns: Vec<Alignment> = table
        .col_specs
        .iter()
        .map(|spec| spec.align.clone())
        .collect();
    let columns = aligns.len();

    let mut grid = RowSpanGrid::new(columns);
    let mut rows: Vec<RenderedRow> = Vec::new();
    for row in &table.head.rows {
        rows.push(place_row(&mut grid, row, columns, true));
    }
    for body in &table.bodies {
        for row in body.head.iter().chain(body.body.iter()) {
            rows.push(place_row(&mut grid, row, columns, false));
        }
    }
    for row in &table.foot.rows {
        rows.push(place_row(&mut grid, row, columns, false));
    }

    let widths = column_widths(&rows, columns);
    let mut out = String::new();
    let caption = caption_lines(&table.caption.long).join("\\\\\n");
    if !caption.is_empty() {
        out.push_str(&caption);
        out.push('\n');
    }
    for (index, row) in rows.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(&render_row(row, &widths, &aligns));
    }
    out
}

/// Flatten caption blocks into inline-markup lines: block structure is discarded (list markers,
/// quote prefixes, and code fences fall away) and each leaf block contributes one line, joined by the
/// caller with the forced-break marker.
fn caption_lines(blocks: &[Block]) -> Vec<String> {
    let mut lines = Vec::new();
    for block in blocks {
        match block {
            Block::Plain(inlines) | Block::Para(inlines) | Block::Header(_, _, inlines) => {
                lines.push(inlines_to_markup(inlines));
            }
            Block::LineBlock(rows) => {
                for row in rows {
                    lines.push(inlines_to_markup(row));
                }
            }
            Block::CodeBlock(_, text) => lines.push(format!("''%%{text}%%''")),
            Block::RawBlock(format, text) => {
                let rendered = raw_passthrough(format, text);
                if !rendered.is_empty() {
                    lines.push(rendered);
                }
            }
            Block::BlockQuote(inner) | Block::Div(_, inner) | Block::Figure(_, _, inner) => {
                lines.extend(caption_lines(inner));
            }
            Block::BulletList(items) | Block::OrderedList(_, items) => {
                for item in items {
                    lines.extend(caption_lines(item));
                }
            }
            Block::DefinitionList(items) => {
                for (term, definitions) in items {
                    lines.push(inlines_to_markup(term));
                    for definition in definitions {
                        lines.extend(caption_lines(definition));
                    }
                }
            }
            Block::Table(table) => lines.extend(caption_lines(&table.caption.long)),
            Block::HorizontalRule => {}
        }
    }
    lines.retain(|line| !line.is_empty());
    lines
}

/// One column slot of a laid-out row: the start of a cell carrying its text, or a filler covered by
/// a column or row span (or a column the row never reached), which renders as empty content.
enum Slot {
    Cell(String),
    Filler,
}

struct RenderedRow {
    header: bool,
    slots: Vec<Slot>,
}

/// Lay out one row over the shared span grid into a fixed-width slot list: each cell carries its
/// rendered text, every column covered by a span (or never reached by the row's cells) is a filler,
/// and the list is padded to `columns` so every row presents the same column count.
fn place_row(grid: &mut RowSpanGrid, row: &Row, columns: usize, header: bool) -> RenderedRow {
    let mut slots: Vec<Slot> = grid
        .place_slots(&row.cells)
        .into_iter()
        .map(|slot| match slot {
            GridSlot::Cell(_, cell) => Slot::Cell(cell_text(cell)),
            GridSlot::Covered => Slot::Filler,
        })
        .collect();
    if slots.len() < columns {
        slots.resize_with(columns, || Slot::Filler);
    }
    RenderedRow { header, slots }
}

/// A cell's content rendered to a single physical line: its blocks rendered as normal markup but
/// with top-level lists flush to the left margin, then every line break folded into the forced-break
/// marker `\\ ` since a cell occupies one row of the source table.
fn cell_text(cell: &Cell) -> String {
    cell.content
        .iter()
        .map(cell_block)
        .filter(|rendered| !rendered.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
        .replace('\n', "\\\\ ")
}

/// Render one block of a table cell. Top-level lists start at the left margin (depth zero) rather
/// than the document list indent; everything else renders as it would outside a cell.
fn cell_block(block: &Block) -> String {
    match block {
        Block::BulletList(items) => bullet_list(items, 0),
        Block::OrderedList(_, items) => ordered_list(items, 0),
        Block::Div(_, blocks) => blocks
            .iter()
            .map(cell_block)
            .filter(|rendered| !rendered.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
        other => self::block(other),
    }
}

/// The width of each column: the longest content cell — measured in characters — that starts in
/// that column.
fn column_widths(rows: &[RenderedRow], columns: usize) -> Vec<usize> {
    let mut widths = vec![0usize; columns];
    for row in rows {
        for (column, slot) in row.slots.iter().enumerate() {
            if let Slot::Cell(text) = slot
                && let Some(width) = widths.get_mut(column)
            {
                *width = (*width).max(text.chars().count());
            }
        }
    }
    widths
}

fn render_row(row: &RenderedRow, widths: &[usize], aligns: &[Alignment]) -> String {
    let separator = if row.header { '^' } else { '|' };
    let mut out = String::new();
    for (column, slot) in row.slots.iter().enumerate() {
        out.push(separator);
        let width = widths.get(column).copied().unwrap_or(0);
        let align = aligns.get(column).unwrap_or(&Alignment::AlignDefault);
        let text = match slot {
            Slot::Cell(text) => text.as_str(),
            Slot::Filler => "",
        };
        out.push_str(&pad_cell(text, width, align));
    }
    out.push(separator);
    out
}

/// Pad cell text to its field width per the column alignment: default fills to the bare width on the
/// right; left and right add a one-space gutter on each side; center adds two.
fn pad_cell(text: &str, width: usize, align: &Alignment) -> String {
    let content = text.chars().count();
    match align {
        Alignment::AlignDefault => {
            let fill = width.saturating_sub(content);
            format!("{text}{}", " ".repeat(fill))
        }
        Alignment::AlignLeft => {
            let fill = (width + 2).saturating_sub(content);
            format!("{text}{}", " ".repeat(fill))
        }
        Alignment::AlignRight => {
            let fill = (width + 2).saturating_sub(content);
            format!("{}{text}", " ".repeat(fill))
        }
        Alignment::AlignCenter => {
            let total = (width + 4).saturating_sub(content);
            let left = total / 2;
            let right = total - left;
            format!("{}{text}{}", " ".repeat(left), " ".repeat(right))
        }
    }
}
