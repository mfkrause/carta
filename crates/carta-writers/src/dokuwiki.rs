//! Render `DokuWiki` markup without a trailing newline.
//!
//! Preserve mode retains soft breaks; other modes convert them to spaces.

use std::fmt::Write as _;

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, Document, Format, Inline, MathType, Row, Table, Target,
    to_plain_text,
};
use carta_core::{Result, WrapMode, Writer, WriterOptions, budget};

use crate::common::{
    self, GridSlot, RawTrim, RowSpanGrid, attribute_value, label_matches_url, quote_marks,
    simple_table_rows, split_length_unit,
};

/// Columns each level of list nesting adds, and the base indent of a top-level list line.
const LIST_INDENT: usize = 2;

// Content breaks stay distinct from block separators until a cell or the document resolves them.
const CONTENT_BREAK: &str = "\u{e000}s";
const FORCED_BREAK: &str = "\u{e000}h";

// Nested table rows must stay distinct from ordinary forced breaks when measuring padding.
const TABLE_BREAK: &str = "\u{e001}";
const TABLE_ROW_END: &str = "\u{e001}\n";

/// Renders a document to `DokuWiki` markup.
#[derive(Debug, Default, Clone, Copy)]
pub struct DokuwikiWriter;

impl Writer for DokuwikiWriter {
    fn render_document(&self, document: &Document, options: &WriterOptions) -> Result<String> {
        let body = sequence(&document.blocks, Context::document(options.wrap));
        let body = body.strip_suffix('\n').unwrap_or(&body);
        // Any content break that survived to the top level stands on its own physical line.
        Ok(body
            .replace(CONTENT_BREAK, "\n")
            .replace(FORCED_BREAK, "\\\\\n")
            .replace(TABLE_ROW_END, "\n")
            .replace(TABLE_BREAK, "\\\\ "))
    }
}

/// The enclosing constructs that decide a block's prefix and trailing spacing.
#[derive(Clone, Copy)]
struct Context {
    wrap: WrapMode,
    /// Enclosing block quotes: paragraphs, code and list markers carry one `>` per level.
    quote_depth: usize,
    /// Enclosing lists: text blocks inside a list item end without a newline of their own.
    list_depth: usize,
    /// Inside a table cell, where sibling blocks are set apart by a blank line.
    in_cell: bool,
}

impl Context {
    fn document(wrap: WrapMode) -> Self {
        Self {
            wrap,
            quote_depth: 0,
            list_depth: 0,
            in_cell: false,
        }
    }

    /// The context of a cell of a table that stands here: the cell leaves any enclosing quote.
    fn cell(self) -> Self {
        Self {
            in_cell: true,
            quote_depth: 0,
            ..self
        }
    }

    fn quoted(self) -> Self {
        Self {
            quote_depth: self.quote_depth.saturating_add(1),
            ..self
        }
    }

    fn unquoted(self) -> Self {
        Self {
            quote_depth: 0,
            ..self
        }
    }

    fn list_item(self) -> Self {
        Self {
            list_depth: self.list_depth.saturating_add(1),
            ..self
        }
    }

    /// The quote marker run that opens a quoted paragraph, code block or list item.
    fn quote_prefix(self) -> String {
        if self.quote_depth == 0 {
            return String::new();
        }
        budget::charge(self.quote_depth.saturating_add(1));
        format!("{} ", ">".repeat(self.quote_depth))
    }

    /// The newline that closes a text block outside a list.
    fn block_end(self) -> &'static str {
        if self.list_depth == 0 { "\n" } else { "" }
    }

    /// The marker indent of a list item at this depth; a table cell takes one level off.
    fn indent(self) -> String {
        let levels = self.list_depth.saturating_sub(usize::from(self.in_cell));
        " ".repeat(levels.saturating_mul(LIST_INDENT))
    }
}

/// Render sibling blocks one per line. A block that ends its own line (a paragraph outside a list,
/// a heading, a code block) therefore leaves a blank line before its successor.
fn flow(blocks: &[Block], context: Context) -> String {
    rendered(blocks, context).collect::<Vec<_>>().join("\n")
}

/// Render sibling blocks as their context spaces them: one per line, or, in a table cell, each
/// without its closing newline and set apart by a blank line.
fn sequence(blocks: &[Block], context: Context) -> String {
    join_blocks(rendered(blocks, context), context)
}

fn rendered(blocks: &[Block], context: Context) -> impl Iterator<Item = String> {
    blocks
        .iter()
        .take_while(|_| !budget::exhausted())
        .map(move |item| block(item, context))
}

fn join_blocks(parts: impl Iterator<Item = String>, context: Context) -> String {
    if context.in_cell {
        parts
            .map(|part| match part.strip_suffix('\n') {
                Some(stripped) => stripped.to_owned(),
                None => part,
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    } else {
        parts.collect::<Vec<_>>().join("\n")
    }
}

fn block(block: &Block, context: Context) -> String {
    carta_core::with_stack(|| match block {
        Block::Plain(inlines) => inlines_to_markup(inlines, context),
        Block::Para(inlines) => format!(
            "{}{}{}",
            context.quote_prefix(),
            inlines_to_markup(inlines, context),
            context.block_end()
        ),
        Block::LineBlock(lines) => format!(
            "{}{}{}",
            context.quote_prefix(),
            line_block(lines, context),
            context.block_end()
        ),
        Block::CodeBlock(attr, text) => {
            format!("{}{}\n", context.quote_prefix(), code_block(attr, text))
        }
        Block::RawBlock(format, text) => raw_passthrough(format, text),
        Block::BlockQuote(blocks) => sequence(blocks, context.quoted()),
        Block::BulletList(items) => bullet_list(items, context),
        Block::OrderedList(_, items) => ordered_list(items, context),
        Block::DefinitionList(items) => definition_list(items, context),
        Block::Header(level, _, inlines) => header(*level, inlines),
        Block::HorizontalRule => "\n----\n".to_owned(),
        Block::Table(table) => render_table(table, context),
        Block::Figure(_, caption, blocks) => figure(caption, blocks, context),
        Block::Div(_, blocks) => container(rendered(blocks, context), context),
    })
}

/// A heading: a run of `=` whose length decreases as the level deepens (level 1 is the widest, and
/// levels past six carry none), with the heading text (markup stripped to plain text) set off by
/// single spaces.
fn header(level: i64, inlines: &[Inline]) -> String {
    let depth = level.clamp(1, 7);
    let equals = "=".repeat(usize::try_from((7 - depth).unsigned_abs()).unwrap_or(usize::MAX));
    let text = bare_inlines(inlines);
    format!("{equals} {text} {equals}\n")
}

/// A line block: its lines on consecutive output lines, each but the last ending in the forced-break
/// marker `\\`.
fn line_block(lines: &[Vec<Inline>], context: Context) -> String {
    lines
        .iter()
        .map(|line| inlines_to_markup(line, context))
        .collect::<Vec<_>>()
        .join(FORCED_BREAK)
}

/// A fenced code block: the verbatim payload between `<code>` tags, the first class naming the
/// source language when present.
fn code_block(attr: &Attr, text: &str) -> String {
    let text = text.strip_suffix('\n').unwrap_or(text);
    match attr.classes.first() {
        Some(language) if !language.is_empty() => format!("<code {language}>\n{text}\n</code>"),
        _ => format!("<code>\n{text}\n</code>"),
    }
}

/// A transparent container of already-rendered blocks. Outside lists and table cells it closes with
/// a newline of its own, so even an empty container holds a line.
fn container(parts: impl Iterator<Item = String>, context: Context) -> String {
    let end = if context.in_cell {
        ""
    } else {
        context.block_end()
    };
    format!("{}{end}", join_blocks(parts, context))
}

/// A figure: a container of its body blocks followed, when a caption is present, by a container of
/// the caption blocks.
fn figure(caption: &Caption, blocks: &[Block], context: Context) -> String {
    let caption =
        (!caption.long.is_empty()).then(|| container(rendered(&caption.long, context), context));
    container(rendered(blocks, context).chain(caption), context)
}

fn bullet_list(items: &[Vec<Block>], context: Context) -> String {
    let inner = context.list_item();
    let prefix = format!("{}{}* ", context.quote_prefix(), inner.indent());
    let body = items
        .iter()
        .map(|item| list_item(item, &prefix, inner))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{body}{}", context.block_end())
}

fn ordered_list(items: &[Vec<Block>], context: Context) -> String {
    let inner = context.list_item();
    let prefix = format!("{}{}-", context.quote_prefix(), inner.indent());
    let body = items
        .iter()
        .map(|item| entry(&prefix, std::slice::from_ref(item), inner))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{body}{}", context.block_end())
}

/// Render bold terms in list items, each followed by its definitions.
fn definition_list(items: &[(Vec<Inline>, Vec<Vec<Block>>)], context: Context) -> String {
    let inner = context.list_item();
    let body = items
        .iter()
        .map(|(term, definitions)| {
            let prefix = format!(
                "{}{}* **{}**",
                context.quote_prefix(),
                inner.indent(),
                inlines_to_markup(term, inner)
            );
            entry(&prefix, definitions, inner)
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("{body}{}", context.block_end())
}

/// Render one bullet item, wrapping complex content in a `<WRAP>` block. A code block that closes a
/// two-block item opens on the line of the block before it.
fn list_item(item: &[Block], prefix: &str, context: Context) -> String {
    let body = match item {
        [first, code @ Block::CodeBlock(..)] => {
            format!("{}{}", block(first, context), block(code, context))
        }
        _ => flow(item, context),
    };
    if is_simple_item(item) {
        format!("{prefix}{body}")
    } else {
        format!("{prefix}<WRAP>\n{body}\n</WRAP>")
    }
}

/// Render the body of a numbered item or of a term's definitions. Simple bodies follow the marker
/// on its line, joined by `; `; otherwise all of them share one `<WRAP>` block. The body leaves any
/// enclosing quote, whose marker the item line already carries.
fn entry(prefix: &str, definitions: &[Vec<Block>], context: Context) -> String {
    let context = context.unquoted();
    let bodies: Vec<String> = definitions
        .iter()
        .map(|definition| sequence(definition, context))
        .collect();
    if definitions
        .iter()
        .all(|definition| is_simple_item(definition))
    {
        format!("{prefix} {}", bodies.join("; "))
    } else {
        format!("{prefix} <WRAP>\n{}\n</WRAP>", bodies.join("\n"))
    }
}

/// Whether a list item fits on its marker line: one text block, optionally followed by a single
/// code block or by any number of simple sublists.
fn is_simple_item(item: &[Block]) -> bool {
    match item {
        [] | [Block::Plain(_) | Block::Para(_), Block::CodeBlock(..)] => true,
        [first, rest @ ..] => is_text_block(first) && rest.iter().all(is_simple_sublist),
    }
}

fn is_text_block(block: &Block) -> bool {
    match block {
        Block::Plain(_) | Block::Para(_) => true,
        Block::Div(_, blocks) => is_simple_item(blocks),
        _ => false,
    }
}

/// Whether a list nested in an item needs no `<WRAP>` around that item: every item of its own is
/// simple, and a numbered list starts at one.
fn is_simple_sublist(block: &Block) -> bool {
    match block {
        Block::BulletList(items) => items.iter().all(|item| is_simple_item(item)),
        Block::OrderedList(attributes, items) => {
            attributes.start == 1 && items.iter().all(|item| is_simple_item(item))
        }
        Block::DefinitionList(items) => items
            .iter()
            .flat_map(|(_, definitions)| definitions)
            .all(|definition| is_simple_item(definition)),
        _ => false,
    }
}

/// Emit a raw-passthrough payload verbatim only when its format is `DokuWiki`; any other target is
/// dropped.
fn raw_passthrough(format: &Format, text: &str) -> String {
    common::raw_passthrough(format, text, "dokuwiki", RawTrim::DropAll)
}

/// Render an inline sequence to markup, collapsing each space or soft break to a single space.
fn inlines_to_markup(inlines: &[Inline], context: Context) -> String {
    inlines.iter().map(|i| inline(i, context)).collect()
}

fn inline(inline: &Inline, context: Context) -> String {
    match inline {
        Inline::Str(text) => escape(text),
        Inline::Emph(inlines) => format!("//{}//", inlines_to_markup(inlines, context)),
        Inline::Strong(inlines) => format!("**{}**", inlines_to_markup(inlines, context)),
        Inline::Underline(inlines) => format!("__{}__", inlines_to_markup(inlines, context)),
        Inline::Strikeout(inlines) => format!("<del>{}</del>", inlines_to_markup(inlines, context)),
        Inline::Superscript(inlines) => {
            format!("<sup>{}</sup>", inlines_to_markup(inlines, context))
        }
        Inline::Subscript(inlines) => format!("<sub>{}</sub>", inlines_to_markup(inlines, context)),
        Inline::SmallCaps(inlines) | Inline::Cite(_, inlines) | Inline::Span(_, inlines) => {
            inlines_to_markup(inlines, context)
        }
        Inline::Quoted(kind, inlines) => {
            let (open, close) = quote_marks(kind);
            format!("{open}{}{close}", inlines_to_markup(inlines, context))
        }
        Inline::Code(_, text) => format!("''%%{}%%''", text.replace('\n', CONTENT_BREAK)),
        Inline::Space => " ".to_owned(),
        Inline::SoftBreak => {
            if context.wrap == WrapMode::Preserve {
                CONTENT_BREAK.to_owned()
            } else {
                " ".to_owned()
            }
        }
        Inline::LineBreak => FORCED_BREAK.to_owned(),
        Inline::Math(kind, text) => match kind {
            MathType::InlineMath => format!("${text}$"),
            MathType::DisplayMath => format!("$${text}$$"),
        },
        Inline::RawInline(format, text) => raw_passthrough(format, text),
        Inline::Link(_, inlines, target) => link(inlines, target, context),
        Inline::Image(attr, inlines, target) => image(attr, inlines, target, context),
        Inline::Note(blocks) => note(blocks, context),
    }
}

/// A footnote, set inline between double parentheses. Its blocks leave any enclosing quote.
fn note(blocks: &[Block], context: Context) -> String {
    format!("(({}))", sequence(blocks, context.unquoted()))
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

/// A link. A `mailto:` link whose label is one text run renders that run in angle brackets. When
/// the label is the destination's visible form (its exact text or its percent-decoded text) and the
/// destination carries no space, the URL stands alone. Otherwise the `[[destination|label]]` form is
/// used, with one leading `/` trimmed from the destination.
fn link(inlines: &[Inline], target: &Target, context: Context) -> String {
    if let [Inline::Str(address)] = inlines
        && target.url.starts_with("mailto:")
    {
        return format!("<{address}>");
    }
    let plain = to_plain_text(inlines);
    if label_matches_url(&plain, &target.url) && !target.url.contains(' ') {
        return target.url.to_string();
    }
    let destination = target.url.strip_prefix('/').unwrap_or(&target.url);
    format!("[[{destination}|{}]]", inlines_to_markup(inlines, context))
}

/// An image: `{{url[?size]|caption}}`, where the caption is the title if set, else the alt text, and
/// the optional size is derived from the `width`/`height` attributes.
fn image(attr: &Attr, inlines: &[Inline], target: &Target, context: Context) -> String {
    let mut head = target.url.clone();
    if let Some(size) = image_size(attr) {
        head.push('?');
        head.push_str(&size);
    }
    let caption = if target.title.is_empty() {
        inlines_to_markup(inlines, context)
    } else {
        target.title.to_string()
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
/// negative, non-finite, unparsable, or in an unsupported unit (e.g. `%`, `ex`) yields `None`.
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

/// Render a table: an optional caption paragraph, then header rows marked with `^` and body rows
/// marked with `|`, every cell padded to its column width per the column's alignment. A cell occupies
/// one row of the source table, so a break within its content cannot stand as a newline; preserved
/// source breaks fold into the forced-break marker `\\ ` instead. Every line ends in a newline, so a
/// table without caption and rows renders nothing.
fn render_table(table: &Table, context: Context) -> String {
    if budget::exhausted() {
        return String::new();
    }
    let aligns: Vec<Alignment> = table
        .col_specs
        .iter()
        .map(|spec| spec.align.clone())
        .collect();
    let columns = aligns.len();

    let mut grid = RowSpanGrid::new(columns);
    let mut rows: Vec<RenderedRow> = Vec::new();
    for (row, header) in simple_table_rows(table) {
        rows.push(place_row(&mut grid, row, columns, header, context));
    }

    if budget::exhausted() {
        return String::new();
    }

    let widths = column_widths(&rows, columns);
    let mut lines = Vec::new();
    let caption = caption_lines(&table.caption.long, context).join(FORCED_BREAK);
    if !caption.is_empty() {
        lines.push(caption);
    }
    lines.extend(rows.iter().map(|row| render_row(row, &widths, &aligns)));
    if lines.is_empty() {
        return String::new();
    }
    let mut out = lines.join(TABLE_ROW_END);
    out.push('\n');
    budget::charge(out.len());
    out
}

/// Flatten caption blocks into inline-markup lines: block structure is discarded (list markers,
/// quote prefixes, and code fences fall away) and each leaf block contributes one line, joined by the
/// caller with the forced-break marker.
fn caption_lines(blocks: &[Block], context: Context) -> Vec<String> {
    let mut lines = Vec::new();
    for block in blocks {
        match block {
            Block::Plain(inlines) | Block::Para(inlines) | Block::Header(_, _, inlines) => {
                lines.push(inlines_to_markup(inlines, context));
            }
            Block::LineBlock(rows) => {
                for row in rows {
                    lines.push(inlines_to_markup(row, context));
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
                lines.extend(caption_lines(inner, context));
            }
            Block::BulletList(items) | Block::OrderedList(_, items) => {
                for item in items {
                    lines.extend(caption_lines(item, context));
                }
            }
            Block::DefinitionList(items) => {
                for (term, definitions) in items {
                    lines.push(inlines_to_markup(term, context));
                    for definition in definitions {
                        lines.extend(caption_lines(definition, context));
                    }
                }
            }
            Block::Table(table) => lines.extend(caption_lines(&table.caption.long, context)),
            Block::HorizontalRule => {}
        }
    }
    lines.retain(|line| !line.is_empty());
    lines
}

/// One column slot of a laid-out row: a cell's single-line text with the width it claims for its
/// column, or a filler covered by a column or row span (or a column the row never reached), which
/// renders as empty content.
enum Slot {
    Cell { text: String, width: usize },
    Filler,
}

struct RenderedRow {
    header: bool,
    slots: Vec<Slot>,
}

/// Lay out one row over the shared span grid into a fixed-width slot list: each cell carries its
/// rendered text, every column covered by a span (or never reached by the row's cells) is a filler,
/// and the list is padded to `columns` so every row presents the same column count.
fn place_row(
    grid: &mut RowSpanGrid,
    row: &Row,
    columns: usize,
    header: bool,
    context: Context,
) -> RenderedRow {
    let mut slots: Vec<Slot> = grid
        .place_slots(&row.cells)
        .into_iter()
        .map(|slot| match slot {
            GridSlot::Cell(_, cell) => cell_slot(cell, context),
            GridSlot::Covered => Slot::Filler,
        })
        .collect();
    if slots.len() < columns {
        slots.resize_with(columns, || Slot::Filler);
    }
    RenderedRow { header, slots }
}

/// A cell's content rendered to a single physical line: its blocks set apart by blank lines, then
/// every line break folded into the forced-break marker `\\ ` since a cell occupies one row of the
/// source table. `context` is that of the table.
///
/// The cell claims its whole text as column width. A cell holding tables nested two deep claims
/// only its longest nested row, so padding cannot compound with the nesting depth.
fn cell_slot(cell: &Cell, context: Context) -> Slot {
    let body = sequence(&cell.content, context.cell());
    let nested_twice = body
        .split(TABLE_ROW_END)
        .any(|row| row.contains(TABLE_BREAK));
    let text = body
        .replace(TABLE_ROW_END, TABLE_BREAK)
        .replace(FORCED_BREAK, "\\\\ ")
        .replace(CONTENT_BREAK, "\\\\ ")
        .replace('\n', "\\\\ ");
    let width = if nested_twice {
        text.split(TABLE_BREAK)
            .map(|line| line.chars().count())
            .max()
            .unwrap_or(0)
    } else {
        text_width(&text)
    };
    Slot::Cell { text, width }
}

/// The columns a cell's text takes once every nested row break is written out as `\\ `.
fn text_width(text: &str) -> usize {
    text.chars().count() + 2 * text.matches(TABLE_BREAK).count()
}

fn column_widths(rows: &[RenderedRow], columns: usize) -> Vec<usize> {
    let mut widths = vec![0usize; columns];
    for row in rows {
        for (column, slot) in row.slots.iter().enumerate() {
            if let Slot::Cell { width: claimed, .. } = slot
                && let Some(width) = widths.get_mut(column)
            {
                *width = (*width).max(*claimed);
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
            Slot::Cell { text, .. } => text.as_str(),
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
    let content = text_width(text);
    let width = width.max(content);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_blocks_preserve_trailing_breaks() {
        for (text, expected) in [
            ("", "<code>\n\n</code>"),
            ("x", "<code>\nx\n</code>"),
            ("x\n", "<code>\nx\n</code>"),
            ("x\n\n", "<code>\nx\n\n</code>"),
            ("\n", "<code>\n\n</code>"),
        ] {
            assert_eq!(code_block(&Attr::default(), text), expected);
        }
        let attr = Attr {
            classes: vec!["rust".into()],
            ..Attr::default()
        };
        assert_eq!(code_block(&attr, "x\n"), "<code rust>\nx\n</code>");
    }

    fn render(inlines: Vec<Inline>) -> String {
        let document = Document {
            blocks: vec![Block::Para(inlines)],
            ..Document::default()
        };
        DokuwikiWriter
            .write(&document, &WriterOptions::default())
            .unwrap()
    }

    fn cell_text(cell: &Cell, wrap: WrapMode) -> String {
        match cell_slot(cell, Context::document(wrap)) {
            Slot::Cell { text, .. } => text,
            Slot::Filler => String::new(),
        }
    }

    fn s(text: &str) -> Inline {
        Inline::Str(text.to_owned().into())
    }

    fn link(label: Vec<Inline>, url: &str) -> Inline {
        Inline::Link(
            Box::default(),
            label,
            Box::new(Target {
                url: url.into(),
                title: String::new().into(),
            }),
        )
    }

    #[test]
    fn decoded_label_link_renders_bare() {
        assert_eq!(
            render(vec![link(
                vec![s("http://e.com/a b")],
                "http://e.com/a%20b"
            )]),
            "http://e.com/a%20b"
        );
    }

    #[test]
    fn exact_label_link_renders_bare() {
        assert_eq!(
            render(vec![link(
                vec![s("http://e.com/a%20b")],
                "http://e.com/a%20b"
            )]),
            "http://e.com/a%20b"
        );
    }

    #[test]
    fn distinct_label_link_renders_explicit() {
        assert_eq!(
            render(vec![link(vec![s("click")], "http://e.com/a%20b")]),
            "[[http://e.com/a%20b|click]]"
        );
    }

    #[test]
    fn mail_links_with_a_phrase_label_keep_their_address() {
        assert_eq!(
            render(vec![link(vec![s("me@e.com")], "mailto:me@e.com")]),
            "<me@e.com>"
        );
        assert_eq!(
            render(vec![link(
                vec![s("mail"), Inline::Space, s("me")],
                "mailto:me@e.com"
            )]),
            "[[mailto:me@e.com|mail me]]"
        );
    }

    #[test]
    fn quoted_figures_keep_body_and_caption_boundaries() {
        for (body, caption, expected) in [
            (Block::Para(vec![s("body")]), Vec::new(), "> body\n"),
            (
                Block::Para(vec![s("body")]),
                vec![Block::Para(vec![s("caption")])],
                "> body\n\n> caption\n\n",
            ),
            (
                Block::Plain(vec![s("body")]),
                vec![Block::Para(vec![s("caption")])],
                "body\n> caption\n\n",
            ),
        ] {
            let document = Document {
                blocks: vec![Block::BlockQuote(vec![Block::Figure(
                    Box::default(),
                    Box::new(Caption {
                        long: caption,
                        ..Caption::default()
                    }),
                    vec![body],
                )])],
                ..Document::default()
            };
            assert_eq!(
                DokuwikiWriter
                    .write(&document, &WriterOptions::default())
                    .unwrap(),
                expected
            );
        }
    }

    #[test]
    fn quoted_code_keeps_its_internal_text() {
        let document = Document {
            blocks: vec![Block::BlockQuote(vec![Block::CodeBlock(
                Box::default(),
                "first\n> literal\nlast".into(),
            )])],
            ..Document::default()
        };
        assert_eq!(
            DokuwikiWriter
                .write(&document, &WriterOptions::default())
                .unwrap(),
            "> <code>\nfirst\n> literal\nlast\n</code>"
        );
    }

    #[test]
    fn deep_quotes_keep_their_depth() {
        let depth = 4096;
        let mut block = Block::Para(vec![s("text")]);
        for _ in 0..depth {
            block = Block::BlockQuote(vec![block]);
        }
        let document = Document {
            blocks: vec![block],
            ..Document::default()
        };
        assert_eq!(
            DokuwikiWriter
                .write(&document, &WriterOptions::default())
                .unwrap(),
            format!("{} text", ">".repeat(depth))
        );
    }

    #[test]
    fn captions_preserve_source_breaks() {
        let document = Document {
            blocks: vec![Block::Table(Box::new(Table {
                caption: Caption {
                    long: vec![Block::Para(vec![s("left"), Inline::SoftBreak, s("right")])],
                    ..Caption::default()
                },
                ..Table::default()
            }))],
            ..Document::default()
        };
        let mut options = WriterOptions::default();
        options.wrap = WrapMode::Preserve;
        assert_eq!(
            DokuwikiWriter.write(&document, &options).unwrap(),
            "left\nright"
        );
    }

    #[test]
    fn hard_breaks_fold_once_and_keep_verbatim_backslashes() {
        let paragraph = vec![s("left"), Inline::LineBreak, s("right")];
        for (content, expected) in [
            (Block::Para(paragraph.clone()), "left\\\\ right"),
            (
                Block::Plain(vec![s("left"), Inline::LineBreak]),
                "left\\\\ ",
            ),
            (
                Block::LineBlock(vec![vec![s("left")], vec![s("right")]]),
                "left\\\\ right",
            ),
            (
                Block::CodeBlock(Box::default(), "left\\\\\nright".into()),
                "<code>\\\\ left\\\\\\\\ right\\\\ </code>",
            ),
            (
                Block::Para(vec![Inline::Code(Box::default(), "left\\\\\nright".into())]),
                "''%%left\\\\\\\\ right%%''",
            ),
            (
                Block::Para(vec![s("left\\\\"), Inline::SoftBreak, s("right")]),
                "left\\\\\\\\ right",
            ),
        ] {
            let cell = Cell {
                attr: Attr::default(),
                align: Alignment::AlignDefault,
                row_span: 1,
                col_span: 1,
                content: vec![content],
            };
            assert_eq!(cell_text(&cell, WrapMode::Preserve), expected);
        }
        for (content, expected) in [
            (Block::Para(paragraph.clone()), "> left\\\\\nright"),
            (Block::Plain(paragraph), "left\\\\\nright"),
            (
                Block::Para(vec![Inline::Code(Box::default(), "left\\\\\nright".into())]),
                "> ''%%left\\\\\nright%%''",
            ),
        ] {
            let document = Document {
                blocks: vec![Block::BlockQuote(vec![content])],
                ..Document::default()
            };
            assert_eq!(
                DokuwikiWriter
                    .write(&document, &WriterOptions::default())
                    .unwrap(),
                expected
            );
        }
    }

    #[test]
    fn cell_definitions_keep_their_structure() {
        let mut cell = Cell {
            attr: Attr::default(),
            align: Alignment::AlignDefault,
            row_span: 1,
            col_span: 1,
            content: vec![Block::DefinitionList(vec![(
                vec![s("term")],
                vec![vec![Block::Para(vec![s("definition")])]],
            )])],
        };
        assert_eq!(cell_text(&cell, WrapMode::Auto), "* **term** definition");
        cell.content = vec![Block::DefinitionList(vec![(
            vec![s("term")],
            vec![Vec::new()],
        )])];
        assert_eq!(cell_text(&cell, WrapMode::Auto), "* **term** ");
        cell.content = vec![Block::DefinitionList(vec![(
            vec![s("term")],
            vec![vec![
                Block::Para(vec![s("first")]),
                Block::Para(vec![s("second")]),
            ]],
        )])];
        assert_eq!(
            cell_text(&cell, WrapMode::Auto),
            "* **term** <WRAP>\\\\ first\\\\ \\\\ second\\\\ </WRAP>"
        );
        cell.content = vec![Block::DefinitionList(vec![(
            vec![s("term")],
            vec![vec![
                Block::Para(vec![s("first")]),
                Block::CodeBlock(Box::default(), "code".into()),
            ]],
        )])];
        assert_eq!(
            cell_text(&cell, WrapMode::Auto),
            "* **term** first\\\\ \\\\ <code>\\\\ code\\\\ </code>"
        );
        assert_eq!(
            block(&cell.content[0], Context::document(WrapMode::Auto)),
            "  * **term** first\n<code>\ncode\n</code>\n\n"
        );
    }

    #[test]
    fn ordered_cells_keep_paragraph_breaks() {
        let items = vec![vec![
            Block::Para(vec![s("first")]),
            Block::Para(vec![s("second")]),
        ]];
        assert_eq!(
            ordered_list(&items, Context::document(WrapMode::Auto).cell()),
            "- <WRAP>\nfirst\n\nsecond\n</WRAP>\n"
        );
    }

    #[test]
    fn numbered_sublists_wrap_when_the_start_changes() {
        for (start, expected) in [
            (1, "  * outer\n    - inner\n"),
            (3, "  * <WRAP>\nouter\n    - inner\n</WRAP>\n"),
        ] {
            let item = vec![
                Block::Plain(vec![s("outer")]),
                Block::OrderedList(
                    carta_ast::ListAttributes {
                        start,
                        style: carta_ast::ListNumberStyle::DefaultStyle,
                        delim: carta_ast::ListNumberDelim::DefaultDelim,
                    },
                    vec![vec![Block::Plain(vec![s("inner")])]],
                ),
            ];
            assert_eq!(
                bullet_list(&[item], Context::document(WrapMode::Auto)),
                expected
            );
        }
    }

    #[test]
    fn nested_table_padding_uses_line_widths() {
        use carta_ast::{ColSpec, ColWidth, TableBody, TableHead};

        fn row(content: Vec<Block>) -> Row {
            Row {
                cells: vec![Cell {
                    attr: Attr::default(),
                    align: Alignment::AlignDefault,
                    row_span: 1,
                    col_span: 1,
                    content,
                }],
                ..Row::default()
            }
        }

        fn nested_table(content: Vec<Block>) -> Block {
            Block::Table(Box::new(Table {
                col_specs: vec![ColSpec {
                    align: Alignment::AlignDefault,
                    width: ColWidth::ColWidthDefault,
                }],
                head: TableHead::default(),
                bodies: vec![TableBody {
                    body: vec![
                        row(content),
                        row(vec![Block::Plain(vec![s("filler")])]),
                        row(vec![Block::Plain(vec![s("filler")])]),
                    ],
                    ..TableBody::default()
                }],
                ..Table::default()
            }))
        }

        let mut block = Block::Plain(vec![s("innermost")]);
        for _ in 0..40 {
            block = nested_table(vec![block]);
        }
        let document = Document {
            blocks: vec![block],
            ..Document::default()
        };
        let rendered = DokuwikiWriter
            .write(&document, &WriterOptions::default())
            .unwrap();
        assert!(rendered.contains("innermost"));
        assert!(rendered.len() < 100_000);
    }
}
