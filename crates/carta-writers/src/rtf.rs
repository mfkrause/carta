//! RTF writer: renders the document model to Rich Text Format.
//!
//! Each block becomes a `{\pard … \par}` paragraph carrying its own alignment, spacing, and left
//! indent, and blocks are concatenated with the paragraph newlines that separate them; the final
//! trailing newline is trimmed since the caller appends its own. Inline emphasis maps to RTF
//! character-formatting groups (`{\i …}`, `{\b …}`, …), literal text is escaped so the control
//! characters `\`, `{`, and `}` render as themselves and every non-ASCII character becomes a
//! `\uN`-escaped UTF-16 code unit, links become `HYPERLINK` fields, and footnotes are emitted inline
//! as auto-numbered `\footnote` groups. Lists thread a left indent that deepens by 360 twips per
//! level and close with the trailing paragraph spacing RTF omits between items. Tables lay their
//! cells out in `\trowd` rows sized in twips. Output carries no trailing newline.
//!
//! Only one extension bears on this format — `east_asian_line_breaks` — and it governs line-fill
//! behavior that RTF does not expose (every paragraph is a single physical line), so it makes no
//! difference to the output and is left unobserved.

use std::fmt::Write as _;
use std::sync::Arc;

use carta_ast::{
    Alignment, Block, Caption, Cell, ColSpec, ColWidth, Document, Inline, ListAttributes, MathType,
    QuoteType, Row, Table, Target,
};
use carta_core::{MediaBag, Result, Writer, WriterOptions};

use crate::common::{
    RawTrim, clean_prefix_len, escape_uri, offset_as_i32, ordered_marker, quote_marks,
    raw_passthrough,
};
use crate::image_size::{image_dimensions, image_dpi};

/// Renders a document to Rich Text Format (no trailing newline).
#[derive(Debug, Default, Clone, Copy)]
pub struct RtfWriter;

impl Writer for RtfWriter {
    fn write(&self, document: &Document, options: &WriterOptions) -> Result<String> {
        let mut state = State::with_media(Arc::clone(&options.media));
        let body = state.blocks(&document.blocks);
        Ok(body.trim_end_matches('\n').to_owned())
    }

    fn render_meta_inlines(&self, inlines: &[Inline], options: &WriterOptions) -> Result<String> {
        let mut state = State::with_media(Arc::clone(&options.media));
        Ok(state.inlines(inlines).trim_end_matches('\n').to_owned())
    }

    fn default_template(&self) -> Option<&'static str> {
        Some(include_str!("templates/default.rtf"))
    }
}

/// The marker and first-line indent a list item's opening paragraph carries: the bullet or numeral
/// glyph plus its tab stop, and the negative first-line indent that hangs the marker to the left of
/// the item text. Set before rendering the item's first block and consumed by the next paragraph.
#[derive(Debug)]
struct Lead {
    first_indent: i32,
    marker: String,
}

/// Writer state threaded through the render: the current left indent (in twips), the alignment
/// control word paragraphs open with, whether paragraphs sit inside a table row, the list nesting
/// depth (which alternates the bullet glyph), any pending list marker the next paragraph consumes,
/// and the resources an embedded image is resolved against.
#[derive(Debug)]
struct State {
    indent: i32,
    align: &'static str,
    in_table: bool,
    list_depth: u32,
    pending: Option<Lead>,
    media: Arc<MediaBag>,
}

impl Default for State {
    fn default() -> Self {
        Self::with_media(Arc::default())
    }
}

impl State {
    fn with_media(media: Arc<MediaBag>) -> Self {
        Self {
            indent: 0,
            align: "\\ql",
            in_table: false,
            list_depth: 0,
            pending: None,
            media,
        }
    }
}

impl State {
    /// Render a block sequence: each block carries its own trailing paragraph newline, so the pieces
    /// concatenate directly.
    fn blocks(&mut self, items: &[Block]) -> String {
        let mut out = String::new();
        for item in items {
            out.push_str(&self.block(item));
        }
        out
    }

    fn block(&mut self, value: &Block) -> String {
        match value {
            Block::Plain(items) => {
                let body = self.inlines(items);
                self.paragraph(0, &body)
            }
            Block::Para(items) => {
                let body = self.inlines(items);
                self.paragraph(180, &body)
            }
            Block::Header(level, _, items) => self.header(*level, items),
            Block::CodeBlock(_, text) => self.code_block(text),
            Block::RawBlock(format, text) => raw_passthrough(format, text, "rtf", RawTrim::DropAll),
            Block::BlockQuote(items) => self.block_quote(items),
            Block::BulletList(items) => self.bullet_list(items),
            Block::OrderedList(attrs, items) => self.ordered_list(attrs, items),
            Block::DefinitionList(items) => self.definition_list(items),
            Block::HorizontalRule => self.horizontal_rule(),
            Block::LineBlock(lines) => self.line_block(lines),
            Block::Table(table) => self.table(table),
            Block::Figure(_, caption, items) => self.figure(caption, items),
            Block::Div(_, items) => self.blocks(items),
        }
    }

    /// Build one paragraph: `{\pard … \par}` carrying the current alignment, the given after-spacing,
    /// the current left indent, and the first-line indent and marker of any pending list lead. Ends
    /// with the newline that separates it from the next block.
    fn paragraph(&mut self, spacing: i32, body: &str) -> String {
        let (first_indent, marker) = match self.pending.take() {
            Some(lead) => (lead.first_indent, lead.marker),
            None => (0, String::new()),
        };
        let table = if self.in_table { "\\intbl" } else { "" };
        format!(
            "{{\\pard{table} {align} \\f0 \\sa{spacing} \\li{indent} \\fi{first_indent} {marker}{body}\\par}}\n",
            align = self.align,
            indent = self.indent,
        )
    }

    fn header(&mut self, level: i32, items: &[Inline]) -> String {
        let outline = level.saturating_sub(1);
        let size = 40i32.saturating_sub(4i32.saturating_mul(level));
        let content = self.inlines(items);
        let body = format!("\\outlinelevel{outline} \\b \\fs{size} {content}");
        self.paragraph(180, &body)
    }

    /// A code block sets the monospace font and preserves its internal line breaks as `\line`, each
    /// followed by a real newline. Trailing blank lines are dropped.
    fn code_block(&mut self, text: &str) -> String {
        let escaped: Vec<String> = text
            .trim_end_matches('\n')
            .split('\n')
            .map(rtf_escape)
            .collect();
        let body = format!("\\f1 {}", escaped.join("\\line\n"));
        self.paragraph(180, &body)
    }

    fn block_quote(&mut self, items: &[Block]) -> String {
        let outer = self.indent;
        self.indent = outer.saturating_add(720);
        let out = self.blocks(items);
        self.indent = outer;
        out
    }

    /// A line block is one paragraph whose lines are joined by forced breaks.
    fn line_block(&mut self, lines: &[Vec<Inline>]) -> String {
        let rendered: Vec<String> = lines.iter().map(|line| self.inlines(line)).collect();
        let body = rendered.join("\\line ");
        self.paragraph(180, &body)
    }

    fn horizontal_rule(&mut self) -> String {
        let outer = self.align;
        self.align = "\\qc";
        let out = self.paragraph(180, "\\emdash\\emdash\\emdash\\emdash\\emdash");
        self.align = outer;
        out
    }

    fn bullet_list(&mut self, items: &[Vec<Block>]) -> String {
        self.list_depth = self.list_depth.saturating_add(1);
        let marker = if self.list_depth % 2 == 1 {
            "\\bullet "
        } else {
            "\\endash "
        };
        let mut out = String::new();
        for item in items {
            out.push_str(&self.list_item(marker, item));
        }
        self.list_depth = self.list_depth.saturating_sub(1);
        append_list_spacing(&mut out);
        out
    }

    fn ordered_list(&mut self, attrs: &ListAttributes, items: &[Vec<Block>]) -> String {
        self.list_depth = self.list_depth.saturating_add(1);
        let mut out = String::new();
        for (offset, item) in items.iter().enumerate() {
            let number = attrs.start.saturating_add(offset_as_i32(offset));
            let marker = ordered_marker(number, attrs.style, attrs.delim);
            out.push_str(&self.list_item(&marker, item));
        }
        self.list_depth = self.list_depth.saturating_sub(1);
        append_list_spacing(&mut out);
        out
    }

    /// Render one list item: its content sits at the surrounding indent plus 360 twips. The first
    /// block opens with the marker and a hanging first-line indent; later blocks continue flush at the
    /// item indent.
    fn list_item(&mut self, marker: &str, item: &[Block]) -> String {
        let outer = self.indent;
        self.indent = outer.saturating_add(360);
        let mut out = String::new();
        // When this item's first block is itself a nested list, that inner item consumes the lead
        // before any text does; carry this marker ahead of the inner one so both glyphs open the line.
        let carried = self
            .pending
            .take()
            .map(|lead| lead.marker)
            .unwrap_or_default();
        self.pending = Some(Lead {
            first_indent: -360,
            marker: format!("{carried}{marker}\\tx360\\tab "),
        });
        match item.split_first() {
            Some((first, rest)) => {
                out.push_str(&self.block(first));
                self.pending = None;
                for block in rest {
                    out.push_str(&self.block(block));
                }
            }
            // An item with no blocks still carries its marker on an otherwise-empty paragraph.
            None => out.push_str(&self.paragraph(0, "")),
        }
        self.pending = None;
        self.indent = outer;
        out
    }

    /// Render a definition list: each term sits at the surrounding indent, its definitions one level
    /// deeper. The list closes with the trailing spacing RTF omits between items.
    fn definition_list(&mut self, items: &[(Vec<Inline>, Vec<Vec<Block>>)]) -> String {
        self.list_depth = self.list_depth.saturating_add(1);
        let mut out = String::new();
        for (term, definitions) in items {
            let body = self.inlines(term);
            out.push_str(&self.paragraph(0, &body));
            let outer = self.indent;
            self.indent = outer.saturating_add(360);
            for definition in definitions {
                out.push_str(&self.blocks(definition));
            }
            self.indent = outer;
        }
        self.list_depth = self.list_depth.saturating_sub(1);
        append_list_spacing(&mut out);
        out
    }

    /// A figure renders its content blocks followed by its caption blocks, with no surrounding chrome.
    fn figure(&mut self, caption: &Caption, items: &[Block]) -> String {
        let mut out = self.blocks(items);
        out.push_str(&self.blocks(&caption.long));
        out
    }

    /// A table lays each row out in a `\trowd` group and always follows the rows with a single caption
    /// paragraph, which is empty when the table carries no caption. Header rows underline their cells.
    /// Rows are expanded to a full grid: a cell that spans columns or rows leaves an empty filler cell
    /// in each further column it covers and in each column of the rows below it continues into, so
    /// every row emits exactly one cell per column. Row spans are tracked within a section (the head,
    /// each body, and the foot each track their own) and do not carry across section boundaries.
    fn table(&mut self, table: &Table) -> String {
        let widths = column_widths(&table.col_specs);
        let columns = table.col_specs.len();
        let mut out = String::new();
        out.push_str(&self.table_section(
            table.head.rows.iter(),
            columns,
            &table.col_specs,
            &widths,
            true,
        ));
        for body in &table.bodies {
            out.push_str(&self.table_section(
                body.head.iter().chain(body.body.iter()),
                columns,
                &table.col_specs,
                &widths,
                false,
            ));
        }
        out.push_str(&self.table_section(
            table.foot.rows.iter(),
            columns,
            &table.col_specs,
            &widths,
            false,
        ));
        let caption = self.inlines(&caption_inlines(&table.caption.long));
        out.push_str(&self.paragraph(180, &caption));
        out
    }

    /// Render one table section (a contiguous run of rows sharing row-span tracking) to its `\trowd`
    /// groups.
    fn table_section<'a>(
        &mut self,
        rows: impl Iterator<Item = &'a Row>,
        columns: usize,
        specs: &[ColSpec],
        widths: &[i64],
        header: bool,
    ) -> String {
        let mut carry = vec![0usize; columns];
        let mut out = String::new();
        for row in rows {
            // A header row with nothing in any cell is layout scaffolding, not content, and is left
            // out entirely rather than drawn as a run of empty bordered cells.
            if header && row.cells.iter().all(cell_is_blank) {
                continue;
            }
            let slots = expand_row(row, columns, &mut carry);
            out.push_str(&self.table_row(&slots, specs, widths, header));
        }
        out
    }

    fn table_row(
        &mut self,
        slots: &[Slot],
        specs: &[ColSpec],
        widths: &[i64],
        header: bool,
    ) -> String {
        let mut cell_defs = String::new();
        for (index, _) in specs.iter().enumerate() {
            if header {
                cell_defs.push_str("\\clbrdrb\\brdrs");
            }
            let _ = write!(
                cell_defs,
                "\\cellx{}",
                widths.get(index).copied().unwrap_or(0)
            );
        }
        let mut cells = String::new();
        for slot in slots {
            match slot {
                Slot::Content { cell, column } => {
                    let column_align = specs.get(*column).map(|spec| &spec.align);
                    cells.push_str(&self.table_cell(&cell.content, &cell.align, column_align));
                }
                Slot::Filler => cells.push_str("{\\cell}\n"),
            }
        }
        format!(
            "{{\n\\trowd \\trgaph120\n{cell_defs}\n\\trkeep\\intbl\n{{\n{cells}}}\n\\intbl\\row}}\n"
        )
    }

    /// Render one cell's content in table mode, at zero indent and the cell's effective alignment: the
    /// cell's own alignment, or the column's when the cell defers.
    fn table_cell(
        &mut self,
        content: &[Block],
        cell_align: &Alignment,
        column_align: Option<&Alignment>,
    ) -> String {
        let effective = match cell_align {
            Alignment::AlignDefault => column_align.unwrap_or(&Alignment::AlignDefault),
            explicit => explicit,
        };
        let saved = (self.align, self.indent, self.in_table);
        self.align = align_word(effective);
        self.indent = 0;
        self.in_table = true;
        let body = self.blocks(content);
        (self.align, self.indent, self.in_table) = saved;
        format!("{{{body}\\cell}}\n")
    }

    fn inlines(&mut self, items: &[Inline]) -> String {
        let mut out = String::new();
        for item in items {
            out.push_str(&self.inline(item));
        }
        out
    }

    #[allow(clippy::match_same_arms)]
    fn inline(&mut self, item: &Inline) -> String {
        match item {
            Inline::Str(text) => rtf_escape(text),
            Inline::Space | Inline::SoftBreak => " ".to_owned(),
            Inline::LineBreak => "\\line ".to_owned(),
            Inline::Emph(items) => self.group("\\i ", items),
            Inline::Strong(items) => self.group("\\b ", items),
            Inline::Underline(items) => self.group("\\ul ", items),
            Inline::Strikeout(items) => self.group("\\strike ", items),
            Inline::Superscript(items) => self.group("\\super ", items),
            Inline::Subscript(items) => self.group("\\sub ", items),
            Inline::SmallCaps(items) => self.group("\\scaps ", items),
            Inline::Code(_, text) => format!("{{\\f1 {}}}", rtf_escape(text)),
            Inline::Quoted(kind, items) => self.quoted(kind, items),
            Inline::Cite(_, items) => self.inlines(items),
            Inline::Math(kind, text) => self.math(kind, text),
            Inline::RawInline(format, text) => raw_passthrough(format, text, "rtf", RawTrim::Keep),
            Inline::Link(_, items, target) => self.link(items, target),
            Inline::Image(_, _, target) => self.image(target),
            Inline::Note(blocks) => self.note(blocks),
            Inline::Span(_, items) => self.inlines(items),
        }
    }

    /// Wrap inline content in an RTF character-formatting group opened by `control`.
    fn group(&mut self, control: &str, items: &[Inline]) -> String {
        format!("{{{control}{}}}", self.inlines(items))
    }

    fn quoted(&mut self, kind: &QuoteType, items: &[Inline]) -> String {
        let (open, close) = quote_marks(kind);
        format!(
            "{}{}{}",
            rtf_escape(&open.to_string()),
            self.inlines(items),
            rtf_escape(&close.to_string())
        )
    }

    /// A link becomes a `HYPERLINK` field; the destination is percent-escaped for URI safety and then
    /// RTF-escaped. The title and attributes are not represented.
    fn link(&mut self, items: &[Inline], target: &Target) -> String {
        let url = rtf_escape(&escape_uri(&target.url));
        let content = self.inlines(items);
        let mut out = String::from("{\\field{\\*\\fldinst{HYPERLINK \"");
        out.push_str(&url);
        out.push_str("\"}}{\\fldrslt{\\ul\n");
        out.push_str(&content);
        out.push_str("\n}}}\n");
        out
    }

    /// A footnote is emitted inline as an auto-numbered `\footnote` group; its body renders at the
    /// document's own indentation, outside any enclosing table row.
    fn note(&mut self, blocks: &[Block]) -> String {
        let saved = (self.indent, self.align, self.in_table, self.list_depth);
        let pending = self.pending.take();
        self.indent = 0;
        self.align = "\\ql";
        self.in_table = false;
        self.list_depth = 0;
        let body = self.blocks(blocks);
        (self.indent, self.align, self.in_table, self.list_depth) = saved;
        self.pending = pending;
        format!("{{\\super\\chftn}}{{\\*\\footnote\\chftn\\~\\plain\\pard {body}}}")
    }

    /// Render math: a single-line-representable expression lowers to the writer-agnostic inline tree
    /// and renders with the inline machinery; anything else is emitted verbatim between the delimiters
    /// of its kind (`$…$` inline, `$$…$$` display) with RTF escaping applied.
    fn math(&mut self, kind: &MathType, text: &str) -> String {
        if let Some(inlines) = crate::math::to_inlines(text) {
            return self.inlines(&inlines);
        }
        let delimiter = match kind {
            MathType::InlineMath => "$",
            MathType::DisplayMath => "$$",
        };
        format!("{delimiter}{}{delimiter}", rtf_escape(text))
    }

    /// An image whose bytes resolve to an embeddable raster becomes a `\pict` group carrying its
    /// pixel size and physical goal; anything the format cannot embed — an unresolved reference or a
    /// raster kind RTF has no blip for — falls back to a bracketed placeholder naming the source.
    fn image(&self, target: &Target) -> String {
        if let Some(bytes) = self.resolve_image(&target.url) {
            if let Some(picture) = pict_group(&bytes) {
                return picture;
            }
        }
        format!("{{\\cf1 [image: {}]\\cf0}}", rtf_escape(&target.url))
    }

    /// The raw bytes an image reference points at: a resource carried in the media bag, or a `data:`
    /// URI decoded inline. A reference to neither yields nothing.
    fn resolve_image(&self, url: &str) -> Option<Vec<u8>> {
        if let Some(item) = self.media.get(url) {
            return Some(item.bytes.clone());
        }
        decode_data_uri(url)
    }
}

/// One column position in an expanded table row: a cell placed at its origin column, or a filler
/// covering a column a neighboring cell spans into (a column span in the same row, or a row span
/// continuing from an earlier row).
enum Slot<'a> {
    Content { cell: &'a Cell, column: usize },
    Filler,
}

/// Expands one row into exactly `columns` slots, threading `carry` — the number of further rows each
/// column stays covered by an active row span. A column still covered from above yields a filler and
/// counts down; otherwise the next cell is placed, its column-span columns become fillers, and every
/// column it occupies is marked covered for the rows its row span reaches.
fn expand_row<'a>(row: &'a Row, columns: usize, carry: &mut [usize]) -> Vec<Slot<'a>> {
    let mut slots = Vec::with_capacity(columns);
    let mut cell_index = 0;
    let mut column = 0;
    while column < columns {
        if carry.get(column).copied().unwrap_or(0) > 0 {
            if let Some(remaining) = carry.get_mut(column) {
                *remaining -= 1;
            }
            slots.push(Slot::Filler);
            column += 1;
            continue;
        }
        let Some(cell) = row.cells.get(cell_index) else {
            slots.push(Slot::Filler);
            column += 1;
            continue;
        };
        cell_index += 1;
        let col_span = usize::try_from(cell.col_span).unwrap_or(1).max(1);
        let row_span = usize::try_from(cell.row_span).unwrap_or(1).max(1);
        let below = row_span.saturating_sub(1);
        let mut span = 0;
        while span < col_span && column < columns {
            if below > 0
                && let Some(remaining) = carry.get_mut(column)
            {
                *remaining = below;
            }
            slots.push(if span == 0 {
                Slot::Content { cell, column }
            } else {
                Slot::Filler
            });
            span += 1;
            column += 1;
        }
    }
    slots
}

/// The eight-byte signature every PNG begins with, and the two-byte one every JPEG does.
const PNG_SIGNATURE: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
const JPEG_SIGNATURE: &[u8] = &[0xff, 0xd8];

/// Encodes an image's bytes as a `\pict` group when it is a raster RTF can embed directly. PNG maps
/// to `\pngblip` and JPEG to `\jpegblip`; each carries its pixel dimensions and a twip goal derived
/// from the image's own resolution, followed by the file bytes as a continuous lowercase-hex run.
/// Dimensions that cannot be read are left off. Any other format returns `None`.
fn pict_group(bytes: &[u8]) -> Option<String> {
    let blip = if bytes.get(..PNG_SIGNATURE.len()) == Some(PNG_SIGNATURE) {
        "\\pngblip"
    } else if bytes.get(..JPEG_SIGNATURE.len()) == Some(JPEG_SIGNATURE) {
        "\\jpegblip"
    } else {
        return None;
    };
    let mut out = format!("{{\\pict{blip}");
    let (width, height) = image_dimensions(bytes);
    if width != 0 && height != 0 {
        let (dpi_x, dpi_y) = image_dpi(bytes);
        let _ = write!(
            out,
            "\\picw{width}\\pich{height}\\picwgoal{}\\pichgoal{}",
            twip_goal(width, dpi_x),
            twip_goal(height, dpi_y),
        );
    }
    out.push(' ');
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out.push('}');
    Some(out)
}

/// The width, in twips, of `pixels` at `dpi`: one inch is 1440 twips. Computed in 64-bit to stay
/// clear of overflow; a zero resolution is treated as one dot per inch rather than dividing by zero.
fn twip_goal(pixels: u32, dpi: u32) -> u64 {
    u64::from(pixels) * 1440 / u64::from(dpi.max(1))
}

/// Decodes a `data:` URI into its raw bytes, for a picture embedded directly in a reference. Only a
/// base64 payload is decoded; a reference that is not a `data:` URI, carries no base64 marker, or
/// holds malformed base64 yields nothing, leaving the image to degrade to its placeholder.
fn decode_data_uri(url: &str) -> Option<Vec<u8>> {
    let rest = url.strip_prefix("data:")?;
    let (header, payload) = rest.split_once(',')?;
    header.strip_suffix(";base64")?;
    carta_core::media::base64_decode(payload)
}

/// Insert the trailing paragraph spacing a list adds before its final `\par`, so nested lists closing
/// on the same paragraph accumulate one span each.
fn append_list_spacing(list: &mut String) {
    if let Some(position) = list.rfind("\\par}") {
        list.insert_str(position, "\\sa180");
    }
}

/// The alignment control word for a column or cell alignment; the default and left both open flush
/// left.
fn align_word(align: &Alignment) -> &'static str {
    match align {
        Alignment::AlignCenter => "\\qc",
        Alignment::AlignRight => "\\qr",
        Alignment::AlignLeft | Alignment::AlignDefault => "\\ql",
    }
}

/// The cumulative right edge of each column in twips, out of a full-width 8640. A column with an
/// explicit fractional width uses it; a defaulted column takes an equal share of the whole.
// Layout arithmetic over bounded fractions summing toward 1.0: rounding to the nearest twip is
// intended, and the product stays well within range.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn column_widths(specs: &[ColSpec]) -> Vec<i64> {
    let count = specs.len();
    if count == 0 {
        return Vec::new();
    }
    let share = 1.0 / count as f64;
    let mut cumulative = 0.0;
    let mut edges = Vec::with_capacity(count);
    for spec in specs {
        cumulative += match spec.width {
            ColWidth::ColWidth(fraction) => fraction,
            ColWidth::ColWidthDefault => share,
        };
        edges.push((cumulative * 8640.0).round() as i64);
    }
    edges
}

/// The inline content of a caption's block sequence, flattening its paragraphs into one run with a
/// forced break between successive paragraphs. A wrapping division contributes the paragraphs nested
/// inside it, so a caption grouped under attributes still yields its text.
fn caption_inlines(blocks: &[Block]) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    collect_caption_inlines(blocks, &mut out);
    out
}

fn collect_caption_inlines(blocks: &[Block], out: &mut Vec<Inline>) {
    for block in blocks {
        match block {
            Block::Plain(items) | Block::Para(items) => {
                if !out.is_empty() {
                    out.push(Inline::LineBreak);
                }
                out.extend(items.iter().cloned());
            }
            Block::Div(_, inner) => collect_caption_inlines(inner, out),
            _ => {}
        }
    }
}

/// Whether a cell carries no visible content: it has no blocks, or only empty paragraphs (directly or
/// inside a division). An all-blank header row is dropped rather than drawn.
fn cell_is_blank(cell: &Cell) -> bool {
    cell.content.iter().all(block_is_blank)
}

fn block_is_blank(block: &Block) -> bool {
    match block {
        Block::Plain(items) | Block::Para(items) => items.is_empty(),
        Block::Div(_, blocks) => blocks.iter().all(block_is_blank),
        _ => false,
    }
}

/// Escape literal text for RTF: the control characters `\`, `{`, and `}` are backslash-escaped; a few
/// typographic characters carry an ASCII fallback after their `\uN` escape; every other non-ASCII
/// character becomes one `\uN ?` escape per UTF-16 code unit (a pair for astral characters).
fn rtf_escape(text: &str) -> String {
    let is_trigger = |byte: u8| matches!(byte, b'\\' | b'{' | b'}') || byte >= 0x80;
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let clean = clean_prefix_len(rest, is_trigger);
        let Some((head, tail)) = rest.split_at_checked(clean) else {
            out.push_str(rest);
            break;
        };
        out.push_str(head);
        let mut chars = tail.chars();
        let Some(ch) = chars.next() else { break };
        match ch {
            '\\' => out.push_str("\\\\"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '\u{2018}' => out.push_str("\\u8216'"),
            '\u{2019}' => out.push_str("\\u8217'"),
            '\u{201c}' => out.push_str("\\u8220\""),
            '\u{201d}' => out.push_str("\\u8221\""),
            '\u{2013}' => out.push_str("\\u8211-"),
            '\u{2014}' => out.push_str("\\u8212-"),
            other if !other.is_ascii() => push_unicode(&mut out, other),
            other => out.push(other),
        }
        rest = chars.as_str();
    }
    out
}

/// Append a non-ASCII character as `\uN ?` escapes: one for a basic-plane character, a UTF-16
/// surrogate pair for an astral one, each unit written as its unsigned decimal value.
fn push_unicode(out: &mut String, ch: char) {
    let code = ch as u32;
    if let Ok(unit) = u16::try_from(code) {
        let _ = write!(out, "\\u{unit} ?");
    } else {
        let scalar = code - 0x1_0000;
        let high = 0xd800 + (scalar >> 10);
        let low = 0xdc00 + (scalar & 0x3ff);
        let _ = write!(out, "\\u{high} ?\\u{low} ?");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use carta_ast::{Attr, Cell, Format, ListNumberDelim, ListNumberStyle, TableHead};

    fn render(blocks: Vec<Block>) -> String {
        let document = Document {
            blocks,
            ..Document::default()
        };
        RtfWriter
            .write(&document, &WriterOptions::default())
            .unwrap()
    }

    fn s(text: &str) -> Inline {
        Inline::Str(text.into())
    }

    fn para(items: Vec<Inline>) -> Block {
        Block::Para(items)
    }

    #[test]
    fn empty_document_is_empty() {
        assert_eq!(render(vec![]), "");
    }

    #[test]
    fn paragraph_and_plain_spacing() {
        assert_eq!(
            render(vec![para(vec![s("hi")])]),
            "{\\pard \\ql \\f0 \\sa180 \\li0 \\fi0 hi\\par}"
        );
        assert_eq!(
            render(vec![Block::Plain(vec![s("hi")])]),
            "{\\pard \\ql \\f0 \\sa0 \\li0 \\fi0 hi\\par}"
        );
    }

    #[test]
    fn header_outline_and_size() {
        assert_eq!(
            render(vec![Block::Header(2, Box::default(), vec![s("H")])]),
            "{\\pard \\ql \\f0 \\sa180 \\li0 \\fi0 \\outlinelevel1 \\b \\fs32 H\\par}"
        );
    }

    #[test]
    fn horizontal_rule_is_centered_em_dashes() {
        assert_eq!(
            render(vec![Block::HorizontalRule]),
            "{\\pard \\qc \\f0 \\sa180 \\li0 \\fi0 \\emdash\\emdash\\emdash\\emdash\\emdash\\par}"
        );
    }

    #[test]
    fn code_block_preserves_lines() {
        assert_eq!(
            render(vec![Block::CodeBlock(Box::default(), "a\nb\n".into())]),
            "{\\pard \\ql \\f0 \\sa180 \\li0 \\fi0 \\f1 a\\line\nb\\par}"
        );
    }

    #[test]
    fn block_quote_indents() {
        assert_eq!(
            render(vec![Block::BlockQuote(vec![para(vec![s("q")])])]),
            "{\\pard \\ql \\f0 \\sa180 \\li720 \\fi0 q\\par}"
        );
    }

    #[test]
    fn line_block_joins_with_breaks() {
        assert_eq!(
            render(vec![Block::LineBlock(
                vec![vec![s("one")], vec![s("two")],]
            )]),
            "{\\pard \\ql \\f0 \\sa180 \\li0 \\fi0 one\\line two\\par}"
        );
    }

    #[test]
    fn bullet_list_marker_and_spacing() {
        assert_eq!(
            render(vec![Block::BulletList(vec![
                vec![Block::Plain(vec![s("a")])],
                vec![Block::Plain(vec![s("b")])],
            ])]),
            "{\\pard \\ql \\f0 \\sa0 \\li360 \\fi-360 \\bullet \\tx360\\tab a\\par}\n\
             {\\pard \\ql \\f0 \\sa0 \\li360 \\fi-360 \\bullet \\tx360\\tab b\\sa180\\par}"
        );
    }

    #[test]
    fn nested_bullets_alternate_and_accumulate_spacing() {
        let inner = Block::BulletList(vec![vec![Block::Plain(vec![s("b")])]]);
        let outer = Block::BulletList(vec![vec![Block::Plain(vec![s("a")]), inner]]);
        assert_eq!(
            render(vec![outer]),
            "{\\pard \\ql \\f0 \\sa0 \\li360 \\fi-360 \\bullet \\tx360\\tab a\\par}\n\
             {\\pard \\ql \\f0 \\sa0 \\li720 \\fi-360 \\endash \\tx360\\tab b\\sa180\\sa180\\par}"
        );
    }

    #[test]
    fn ordered_list_numbers() {
        let attrs = ListAttributes {
            start: 1,
            style: ListNumberStyle::Decimal,
            delim: ListNumberDelim::Period,
        };
        assert_eq!(
            render(vec![Block::OrderedList(
                attrs,
                vec![vec![Block::Plain(vec![s("x")])]]
            )]),
            "{\\pard \\ql \\f0 \\sa0 \\li360 \\fi-360 1.\\tx360\\tab x\\sa180\\par}"
        );
    }

    #[test]
    fn definition_list_term_and_definition() {
        assert_eq!(
            render(vec![Block::DefinitionList(vec![(
                vec![s("T")],
                vec![vec![Block::Plain(vec![s("d")])]],
            )])]),
            "{\\pard \\ql \\f0 \\sa0 \\li0 \\fi0 T\\par}\n\
             {\\pard \\ql \\f0 \\sa0 \\li360 \\fi0 d\\sa180\\par}"
        );
    }

    #[test]
    fn inline_styles_and_nesting() {
        assert_eq!(
            render(vec![para(vec![Inline::Strong(vec![Inline::Emph(vec![
                s("x")
            ])])])]),
            "{\\pard \\ql \\f0 \\sa180 \\li0 \\fi0 {\\b {\\i x}}\\par}"
        );
        assert_eq!(
            render(vec![para(vec![Inline::Code(Box::default(), "c".into())])]),
            "{\\pard \\ql \\f0 \\sa180 \\li0 \\fi0 {\\f1 c}\\par}"
        );
    }

    #[test]
    fn quoted_uses_escaped_curly_quotes() {
        assert_eq!(
            render(vec![para(vec![Inline::Quoted(
                QuoteType::DoubleQuote,
                vec![s("q")]
            )])]),
            "{\\pard \\ql \\f0 \\sa180 \\li0 \\fi0 \\u8220\"q\\u8221\"\\par}"
        );
    }

    #[test]
    fn line_break_is_forced() {
        assert_eq!(
            render(vec![para(vec![s("a"), Inline::LineBreak, s("b")])]),
            "{\\pard \\ql \\f0 \\sa180 \\li0 \\fi0 a\\line b\\par}"
        );
    }

    #[test]
    fn escaping_controls_and_unicode() {
        assert_eq!(
            render(vec![para(vec![s("a{b}c\\d")])]),
            "{\\pard \\ql \\f0 \\sa180 \\li0 \\fi0 a\\{b\\}c\\\\d\\par}"
        );
        assert_eq!(
            render(vec![para(vec![s("é…")])]),
            "{\\pard \\ql \\f0 \\sa180 \\li0 \\fi0 \\u233 ?\\u8230 ?\\par}"
        );
        // Astral characters split into a UTF-16 surrogate pair.
        assert_eq!(
            render(vec![para(vec![s("\u{1F600}")])]),
            "{\\pard \\ql \\f0 \\sa180 \\li0 \\fi0 \\u55357 ?\\u56832 ?\\par}"
        );
    }

    #[test]
    fn link_becomes_hyperlink_field() {
        let target = Box::new(Target {
            url: "http://e.com/a b".into(),
            title: "t".into(),
        });
        assert_eq!(
            render(vec![para(vec![Inline::Link(
                Box::default(),
                vec![s("text")],
                target
            )])]),
            "{\\pard \\ql \\f0 \\sa180 \\li0 \\fi0 \
             {\\field{\\*\\fldinst{HYPERLINK \"http://e.com/a%20b\"}}\
             {\\fldrslt{\\ul\ntext\n}}}\n\\par}"
        );
    }

    #[test]
    fn image_shows_source() {
        let target = Box::new(Target {
            url: "img.png".into(),
            title: "".into(),
        });
        assert_eq!(
            render(vec![para(vec![Inline::Image(
                Box::default(),
                vec![s("alt")],
                target
            )])]),
            "{\\pard \\ql \\f0 \\sa180 \\li0 \\fi0 {\\cf1 [image: img.png]\\cf0}\\par}"
        );
    }

    #[test]
    fn footnote_is_inline_group() {
        assert_eq!(
            render(vec![para(vec![
                s("x"),
                Inline::Note(vec![para(vec![s("n")])])
            ])]),
            "{\\pard \\ql \\f0 \\sa180 \\li0 \\fi0 x\
             {\\super\\chftn}{\\*\\footnote\\chftn\\~\\plain\\pard \
             {\\pard \\ql \\f0 \\sa180 \\li0 \\fi0 n\\par}\n}\\par}"
        );
    }

    #[test]
    fn raw_block_rtf_passes_through_others_dropped() {
        assert_eq!(
            render(vec![Block::RawBlock(
                Format("rtf".into()),
                "{\\x}\n".into()
            )]),
            "{\\x}"
        );
        assert_eq!(
            render(vec![
                Block::RawBlock(Format("html".into()), "<div>".into()),
                para(vec![s("y")]),
            ]),
            "{\\pard \\ql \\f0 \\sa180 \\li0 \\fi0 y\\par}"
        );
    }

    #[test]
    fn table_rows_and_caption() {
        let cell = |text: &str, align: Alignment| Cell {
            attr: Attr::default(),
            align,
            row_span: 1,
            col_span: 1,
            content: vec![Block::Plain(vec![s(text)])],
        };
        let spec = |align: Alignment| ColSpec {
            align,
            width: ColWidth::ColWidthDefault,
        };
        let table = Table {
            col_specs: vec![spec(Alignment::AlignLeft), spec(Alignment::AlignRight)],
            head: TableHead {
                attr: Attr::default(),
                rows: vec![Row {
                    attr: Attr::default(),
                    cells: vec![
                        cell("A", Alignment::AlignDefault),
                        cell("B", Alignment::AlignDefault),
                    ],
                }],
            },
            ..Table::default()
        };
        assert_eq!(
            render(vec![Block::Table(Box::new(table))]),
            "{\n\\trowd \\trgaph120\n\
             \\clbrdrb\\brdrs\\cellx4320\\clbrdrb\\brdrs\\cellx8640\n\
             \\trkeep\\intbl\n{\n\
             {{\\pard\\intbl \\ql \\f0 \\sa0 \\li0 \\fi0 A\\par}\n\\cell}\n\
             {{\\pard\\intbl \\qr \\f0 \\sa0 \\li0 \\fi0 B\\par}\n\\cell}\n\
             }\n\\intbl\\row}\n\
             {\\pard \\ql \\f0 \\sa180 \\li0 \\fi0 \\par}"
        );
    }

    #[test]
    fn meta_inlines_have_no_paragraph_chrome() {
        let rendered = RtfWriter
            .render_meta_inlines(&[Inline::Emph(vec![s("Title")])], &WriterOptions::default())
            .unwrap();
        assert_eq!(rendered, "{\\i Title}");
    }
}
