//! Render the document model as `AsciiDoc` markup without a trailing newline.

use std::fmt::Write as _;

use carta_ast::{
    Attr, Block, Caption, Cell, Document, Inline, ListAttributes, MathType, Row, Table, TableBody,
    Target, Text, single_block_inlines, slug, to_plain_text,
};
use carta_core::{Result, TocStyle, WrapMode, Writer, WriterOptions, budget};

use crate::common::{
    self, FILL_COLUMN, Piece, RawTrim, RowSpanGrid, display_width, fill, fill_hang, fill_offset,
    head_rows, label_matches_url,
};

mod helpers;

use self::helpers::{
    alignment_operator, cell_span, code_block, display_math_block, escape_text, flush_left,
    holds_display_math, image_args, indent, is_autolink_scheme, is_simple_cell, joins_word,
    labeled_block, lift_display_math, ordered_style_line, quote_glyphs, reads_as_markup, span_role,
    table_options_line, task_checkbox, trim_surrounding_space, url_scheme, without_flush_marks,
};

/// Renders a document to `AsciiDoc` markup.
#[derive(Debug, Default, Clone, Copy)]
pub struct AsciidocWriter;

impl Writer for AsciidocWriter {
    fn render_document(&self, document: &Document, options: &WriterOptions) -> Result<String> {
        render_document(document, options, false)
    }

    fn default_template(&self) -> Option<&'static str> {
        Some(include_str!("templates/default.asciidoc"))
    }

    fn body_ends_with_newline(&self) -> bool {
        true
    }

    fn toc_style(&self) -> TocStyle {
        TocStyle::Native
    }
}

/// Renders a document with legacy `AsciiDoc` syntax.
#[derive(Debug, Default, Clone, Copy)]
pub struct AsciidocLegacyWriter;

impl Writer for AsciidocLegacyWriter {
    fn render_document(&self, document: &Document, options: &WriterOptions) -> Result<String> {
        render_document(document, options, true)
    }

    fn default_template(&self) -> Option<&'static str> {
        AsciidocWriter.default_template()
    }

    fn body_ends_with_newline(&self) -> bool {
        true
    }

    fn toc_style(&self) -> TocStyle {
        TocStyle::Native
    }
}

fn render_document(document: &Document, options: &WriterOptions, legacy: bool) -> Result<String> {
    budget::scope(document, || {
        let mut state = State {
            wrap: options.wrap,
            legacy,
            ..State::default()
        };
        let width = options.columns.unwrap_or(FILL_COLUMN);
        let body = state.blocks(&document.blocks, width);
        Ok(body.trim_end_matches('\n').to_owned())
    })
}

#[derive(Debug, Default)]
struct State {
    bullet_depth: usize,
    ordered_depth: usize,
    definition_depth: usize,
    table_depth: usize,
    wrap: WrapMode,
    legacy: bool,
}

impl State {
    /// Render a top-level block sequence: blocks separated by a single blank line, with empties
    /// dropped.
    fn blocks(&mut self, blocks: &[Block], width: usize) -> String {
        let rendered: Vec<String> = blocks
            .iter()
            .map(|block| self.block(block, width))
            .filter(|text| !text.is_empty())
            .collect();
        rendered.join("\n\n")
    }

    fn block(&mut self, block: &Block, width: usize) -> String {
        match block {
            Block::Plain(inlines) => self.paragraph(inlines, false, width, 0),
            Block::Para(inlines) => self.paragraph(inlines, true, width, 0),
            Block::Header(level, attr, inlines) => self.header(*level, attr, inlines),
            Block::CodeBlock(attr, text) => {
                let block = code_block(attr, text);
                // Verbatim lines keep column zero under a definition's indent.
                if self.definition_depth == 0 {
                    block
                } else {
                    flush_left(&block)
                }
            }
            Block::RawBlock(format, text) => {
                common::raw_passthrough(format, text, "asciidoc", RawTrim::DropOne)
            }
            Block::BlockQuote(blocks) => {
                let body = self.blocks(blocks, width);
                let trimmed = body.trim_end_matches('\n');
                if blocks
                    .iter()
                    .any(|block| matches!(block, Block::BlockQuote(_)))
                {
                    format!("____\n--\n{trimmed}\n\n--\n____")
                } else {
                    format!("____\n{trimmed}\n____")
                }
            }
            Block::BulletList(items) => self.bullet_list(items, width),
            Block::OrderedList(attrs, items) => self.ordered_list(attrs, items, width),
            Block::DefinitionList(items) => self.definition_list(items, width),
            Block::HorizontalRule => "'''''".to_owned(),
            Block::Table(table) => self.table(table, width),
            Block::Figure(attr, caption, blocks) => self.figure(attr, caption, blocks, width),
            Block::Div(attr, blocks) => self.div(attr, blocks, width),
            Block::LineBlock(lines) => self.line_block(lines),
        }
    }

    /// Render a text block. A `guarded` one gets an empty attribute reference in front when its
    /// text would otherwise open a list item or stand as a block attribute line.
    fn paragraph(
        &mut self,
        inlines: &[Inline],
        guarded: bool,
        width: usize,
        initial: usize,
    ) -> String {
        if !holds_display_math(inlines) {
            let mut pieces = self.pieces(inlines);
            if guarded && reads_as_markup(&pieces, self.wrap) {
                pieces.insert(0, Piece::text("{empty}"));
            }
            return fill_offset(&pieces, width, initial, self.wrap);
        }
        let mut out = String::new();
        let flush_text = |state: &mut Self, run: &[Inline], out: &mut String| {
            let offset = if out.is_empty() { initial } else { 0 };
            if let Some(rendered) = state.text_segment(run, width, offset) {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&rendered);
            }
        };
        let mut lifted = Vec::new();
        lift_display_math(inlines, &mut lifted);
        let mut run_start = 0;
        for (index, inline) in lifted.iter().enumerate() {
            let Inline::Math(MathType::DisplayMath, math) = inline else {
                continue;
            };
            flush_text(self, lifted.get(run_start..index).unwrap_or(&[]), &mut out);
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(&display_math_block(math, self.legacy));
            run_start = index + 1;
        }
        flush_text(self, lifted.get(run_start..).unwrap_or(&[]), &mut out);
        out
    }

    fn text_segment(&mut self, inlines: &[Inline], width: usize, initial: usize) -> Option<String> {
        let trimmed = trim_surrounding_space(inlines);
        if trimmed.is_empty() {
            return None;
        }
        let rendered = fill_offset(&self.pieces(trimmed), width, initial, self.wrap);
        (!rendered.is_empty()).then_some(rendered)
    }

    fn header(&mut self, level: i64, attr: &Attr, inlines: &[Inline]) -> String {
        let depth = usize::try_from(level.max(0))
            .unwrap_or(usize::MAX)
            .saturating_add(1);
        budget::charge(depth);
        if budget::exhausted() {
            return String::new();
        }
        let equals = "=".repeat(depth);
        // A section title is a single line: it is never reflowed, whatever the wrap mode.
        let text = self.inline_string(inlines);
        let heading = format!("{equals} {text}");
        if !attr.id.is_empty() && attr.id != slug(&to_plain_text(inlines)) {
            format!("[[{}]]\n{heading}", attr.id)
        } else {
            heading
        }
    }

    fn line_block(&mut self, lines: &[Vec<Inline>]) -> String {
        let body: Vec<String> = lines
            .iter()
            .map(|line| fill_hang(&self.pieces(line), usize::MAX, self.wrap))
            .collect();
        format!("[verse]\n--\n{}\n--", body.join("\n"))
    }

    fn div(&mut self, attr: &Attr, blocks: &[Block], width: usize) -> String {
        let Some((label, fence)) = labeled_block(attr) else {
            return anchored(&attr.id, self.blocks(blocks, width));
        };
        let (title, content) = match blocks {
            [Block::Div(title_attr, title), rest @ ..]
                if title_attr.classes.first().map(Text::as_str) == Some("title") =>
            {
                (title.as_slice(), rest)
            }
            _ => (&[][..], blocks),
        };
        let mut out = format!("[{label}]\n");
        // A title that only repeats the label adds nothing.
        let repeats_label = to_plain_text(single_block_inlines(title))
            .trim()
            .eq_ignore_ascii_case(label);
        let title = self.blocks(title, width);
        if !title.is_empty() && !repeats_label {
            let _ = writeln!(out, ".{title}");
        }
        let _ = writeln!(out, "{fence}");
        let body = self.blocks(content, width);
        if !body.is_empty() {
            let _ = writeln!(out, "{}", body.trim_end_matches('\n'));
        }
        out.push_str(fence);
        anchored(&attr.id, out)
    }

    fn figure(&mut self, attr: &Attr, caption: &Caption, blocks: &[Block], width: usize) -> String {
        let block_image = match blocks {
            [Block::Plain(inlines) | Block::Para(inlines)] => match inlines.as_slice() {
                [Inline::Image(image_attr, alt, target)] => Some((image_attr, alt, target)),
                _ => None,
            },
            _ => None,
        };
        let Some((image_attr, alt, target)) = block_image else {
            return anchored(&attr.id, self.blocks(blocks, width));
        };
        let mut out = String::new();
        if let Some(text) = render_caption(caption, self, width) {
            let _ = writeln!(out, ".{text}");
        }
        if !attr.id.is_empty() {
            let _ = writeln!(out, "[#{}]", attr.id);
        }
        let alt_text = self.inline_string(alt);
        let _ = write!(
            out,
            "image::{}[{}]",
            target.url,
            image_args(image_attr, target, &alt_text)
        );
        out
    }

    fn bullet_list(&mut self, items: &[Vec<Block>], width: usize) -> String {
        self.bullet_depth = self.bullet_depth.saturating_add(1);
        let marker = "*".repeat(self.bullet_depth);
        let rendered = self.list_items(items, &marker, None, true, width);
        self.bullet_depth = self.bullet_depth.saturating_sub(1);
        rendered
    }

    fn ordered_list(
        &mut self,
        attrs: &ListAttributes,
        items: &[Vec<Block>],
        width: usize,
    ) -> String {
        self.ordered_depth = self.ordered_depth.saturating_add(1);
        let marker = ".".repeat(self.ordered_depth);
        let style_line = ordered_style_line(attrs);
        let rendered = self.list_items(items, &marker, style_line, false, width);
        self.ordered_depth = self.ordered_depth.saturating_sub(1);
        rendered
    }

    /// Render the items of a bullet or ordered list. `marker` is the depth-repeated leading marker;
    /// `style` is an optional attribute line (`[arabic]`, …) emitted once before the items.
    fn list_items(
        &mut self,
        items: &[Vec<Block>],
        marker: &str,
        style: Option<String>,
        task_markers: bool,
        width: usize,
    ) -> String {
        let rendered: Vec<String> = items
            .iter()
            .map(|item| self.list_item(item, marker, task_markers, width))
            .collect();
        let body = rendered.join("\n");
        match style {
            Some(line) => format!("{line}\n{body}"),
            None => body,
        }
    }

    fn list_item(
        &mut self,
        item: &[Block],
        marker: &str,
        task_markers: bool,
        width: usize,
    ) -> String {
        let starts_with_paragraph = matches!(item.first(), Some(Block::Plain(_) | Block::Para(_)));
        let mut out = if starts_with_paragraph {
            String::new()
        } else {
            format!("{marker} {{blank}}")
        };
        for (index, block) in item.iter().enumerate() {
            let is_list = matches!(block, Block::BulletList(_) | Block::OrderedList(..));
            let rendered = if index == 0
                && let Block::Plain(inlines) | Block::Para(inlines) = block
            {
                let initial = marker.len() + 1;
                let checkbox = (task_markers && !self.legacy)
                    .then(|| task_checkbox(block))
                    .flatten();
                let text = if let Some((box_marker, rest)) = checkbox {
                    let mut body = vec![
                        Inline::RawInline(carta_ast::Format("asciidoc".into()), box_marker.into()),
                        Inline::Space,
                    ];
                    body.extend(rest);
                    self.paragraph(&body, false, width, initial)
                } else {
                    let guarded = matches!(block, Block::Para(_));
                    self.paragraph(inlines, guarded, width, initial)
                };
                format!("{marker} {text}")
            } else {
                self.block(block, width)
            };
            if out.is_empty() {
                out.push_str(&rendered);
                continue;
            }
            if is_list {
                out.push('\n');
            } else {
                out.push_str("\n+\n");
                if matches!(block, Block::HorizontalRule) {
                    out.push('\n');
                }
            }
            out.push_str(&rendered);
        }
        out.truncate(out.trim_end_matches('\n').len());
        out
    }

    fn definition_list(
        &mut self,
        items: &[(Vec<Inline>, Vec<Vec<Block>>)],
        width: usize,
    ) -> String {
        self.definition_depth = self.definition_depth.saturating_add(1);
        // Alternating markers keep a nested list apart from the one that holds it.
        let marker = if self.definition_depth % 2 == 1 {
            "::"
        } else {
            ";;"
        };
        let mut entries = Vec::new();
        for (term, definitions) in items {
            let term_text = fill(&self.pieces(term), width, self.wrap);
            let mut entry = format!("{term_text}{marker}");
            let bodies: Vec<String> = definitions
                .iter()
                .map(|definition| self.definition_body(definition, width))
                .filter(|text| !text.is_empty())
                .collect();
            let joined = bodies.join("\n  +\n");
            if !joined.is_empty() {
                let _ = write!(entry, "\n{joined}");
            }
            entries.push(entry);
        }
        self.definition_depth = self.definition_depth.saturating_sub(1);
        let list = entries.join("\n");
        if self.definition_depth == 0 {
            without_flush_marks(&list)
        } else {
            list
        }
    }

    /// Render a definition's blocks indented two columns, joining successive blocks with a `+`
    /// continuation line.
    fn definition_body(&mut self, blocks: &[Block], width: usize) -> String {
        let body_width = width.saturating_sub(2);
        let rendered: Vec<String> = blocks
            .iter()
            .map(|block| self.block(block, body_width))
            .filter(|text| !text.is_empty())
            .collect();
        let joined = rendered.join("\n+\n");
        indent(&joined, "  ")
    }

    fn table(&mut self, table: &Table, width: usize) -> String {
        let columns = table.col_specs.len();
        let header_rows: Vec<&Row> = head_rows(table).iter().collect();
        let mut out = String::new();
        if let Some(caption) = render_caption(&table.caption, self, width) {
            let _ = writeln!(out, ".{caption}");
        }
        out.push_str(&table_options_line(
            table,
            !header_rows.is_empty(),
            !table.foot.rows.is_empty(),
        ));
        self.table_depth = self.table_depth.saturating_add(1);
        let separator = self.cell_separator();
        let _ = write!(out, "\n{separator}===");
        let mut grid = RowSpanGrid::new(columns);
        for row in &header_rows {
            let _ = write!(out, "\n{}", self.table_row(row, &mut grid, width));
        }
        for body in &table.bodies {
            let _ = write!(out, "{}", self.body_rows(body, &mut grid, width));
        }
        for row in &table.foot.rows {
            let _ = write!(out, "\n{}", self.table_row(row, &mut grid, width));
        }
        let _ = write!(out, "\n{separator}===");
        self.table_depth = self.table_depth.saturating_sub(1);
        out
    }

    /// The cell separator of the table being written: a table inside a cell takes `!` so that its
    /// cells do not end the cell that holds it.
    fn cell_separator(&self) -> char {
        if self.table_depth > 1 { '!' } else { '|' }
    }

    fn body_rows(&mut self, body: &TableBody, grid: &mut RowSpanGrid, width: usize) -> String {
        let mut out = String::new();
        for row in body.head.iter().chain(body.body.iter()) {
            let _ = write!(out, "\n{}", self.table_row(row, grid, width));
        }
        out
    }

    fn table_row(&mut self, row: &Row, grid: &mut RowSpanGrid, width: usize) -> String {
        let mut line = String::new();
        let mut column: usize = 0;
        for (index, (_, cell)) in grid.place(&row.cells).into_iter().enumerate() {
            if index > 0 {
                if line.ends_with('\n') {
                    line.push('\n');
                    column = 0;
                } else {
                    line.push(' ');
                    column = column.saturating_add(1);
                }
            }
            let rendered = self.table_cell(cell, column, width);
            column = match rendered.rsplit_once('\n') {
                Some((_, last)) => display_width(last),
                None => column.saturating_add(display_width(&rendered)),
            };
            line.push_str(&rendered);
        }
        line
    }

    fn table_cell(&mut self, cell: &Cell, column: usize, width: usize) -> String {
        let mut marker = String::new();
        let span = cell_span(cell);
        if !span.is_empty() {
            marker.push_str(&span);
            marker.push('+');
        }
        if let Some(operator) = alignment_operator(&cell.align) {
            marker.push(operator);
        }
        let separator = self.cell_separator();
        if is_simple_cell(&cell.content) {
            let prefix = format!("{marker}{separator}");
            let initial = column.saturating_add(display_width(&prefix));
            let body = self.cell_inlines(&cell.content, width, initial);
            format!("{prefix}{body}")
        } else {
            let body = self.blocks(&cell.content, width);
            let body = body.trim_end_matches('\n');
            if body.is_empty() {
                format!("{marker}a{separator}")
            } else {
                format!("{marker}a{separator}\n{body}\n")
            }
        }
    }

    fn cell_inlines(&mut self, blocks: &[Block], width: usize, initial: usize) -> String {
        match blocks {
            [Block::Plain(inlines) | Block::Para(inlines)] => {
                fill_offset(&self.pieces(inlines), width, initial, self.wrap)
            }
            _ => String::new(),
        }
    }

    fn pieces(&mut self, inlines: &[Inline]) -> Vec<Piece> {
        let mut out = Vec::new();
        for (index, inline) in inlines.iter().enumerate() {
            let before = inlines.get(index.wrapping_sub(1)).filter(|_| index > 0);
            let after = inlines.get(index + 1);
            self.inline(inline, before, after, &mut out);
        }
        out
    }

    fn inline(
        &mut self,
        inline: &Inline,
        before: Option<&Inline>,
        after: Option<&Inline>,
        out: &mut Vec<Piece>,
    ) {
        let text = match inline {
            Inline::Str(text) => escape_text(text),
            Inline::Emph(inlines) => {
                let marker = if joins_word(before, after) { "__" } else { "_" };
                self.wrapped(trim_surrounding_space(inlines), marker, marker, out);
                return;
            }
            Inline::Strong(inlines) => {
                let marker = if joins_word(before, after) { "**" } else { "*" };
                self.wrapped(trim_surrounding_space(inlines), marker, marker, out);
                return;
            }
            Inline::Strikeout(inlines) => {
                self.wrapped(trim_surrounding_space(inlines), "[line-through]#", "#", out);
                return;
            }
            Inline::Underline(inlines) => {
                self.wrapped(trim_surrounding_space(inlines), "[.underline]#", "#", out);
                return;
            }
            Inline::SmallCaps(inlines) => {
                self.wrapped(trim_surrounding_space(inlines), "[smallcaps]#", "#", out);
                return;
            }
            Inline::Superscript(inlines) => format!("^{}^", self.inline_string(inlines)),
            Inline::Subscript(inlines) => format!("~{}~", self.inline_string(inlines)),
            Inline::Quoted(kind, inlines) => {
                let (open, close) = quote_glyphs(kind, self.legacy);
                self.wrapped(inlines, open, close, out);
                return;
            }
            Inline::Cite(_, inlines) => {
                for (index, child) in inlines.iter().enumerate() {
                    let child_before = inlines.get(index.wrapping_sub(1)).filter(|_| index > 0);
                    let child_after = inlines.get(index + 1);
                    self.inline(child, child_before, child_after, out);
                }
                return;
            }
            Inline::Code(_, text) => {
                let code = if self.legacy {
                    text.replace('`', "\\'")
                } else {
                    escape_text(text)
                };
                format!("`{code}`")
            }
            Inline::Space => {
                out.push(Piece::Space);
                return;
            }
            Inline::SoftBreak => {
                out.push(Piece::Soft);
                return;
            }
            Inline::LineBreak => {
                out.push(Piece::text(" +"));
                out.push(Piece::Hard);
                return;
            }
            Inline::Math(_, text) => {
                if self.legacy {
                    format!("latexmath:[${text}$]")
                } else {
                    format!("latexmath:[{text}]")
                }
            }
            Inline::RawInline(format, text) => {
                let rendered = common::raw_passthrough(format, text, "asciidoc", RawTrim::DropOne);
                if !rendered.is_empty() {
                    out.push(Piece::text(rendered));
                }
                return;
            }
            Inline::Link(_, inlines, target) => {
                self.link(inlines, target, out);
                return;
            }
            Inline::Image(attr, inlines, target) => {
                let alt = self.inline_string(inlines);
                format!("image:{}[{}]", target.url, image_args(attr, target, &alt))
            }
            Inline::Span(attr, inlines) => {
                match span_role(attr) {
                    Some(role) => {
                        let marker = if joins_word(before, after) { "##" } else { "#" };
                        self.wrapped(inlines, &format!("{role}{marker}"), marker, out);
                    }
                    None => out.append(&mut self.pieces(inlines)),
                }
                return;
            }
            Inline::Note(blocks) => {
                self.note(blocks, out);
                return;
            }
        };
        out.push(Piece::text(text));
    }

    /// Render an inline sequence between `open` and `close` delimiters. The delimiters fuse to the
    /// adjacent words, and spaces inside the sequence stay wrap points.
    fn wrapped(&mut self, inlines: &[Inline], open: &str, close: &str, out: &mut Vec<Piece>) {
        out.push(Piece::text(open));
        out.append(&mut self.pieces(inlines));
        out.push(Piece::text(close));
    }

    /// Render an inline sequence to a single string with no line filling: spaces stay literal.
    fn inline_string(&mut self, inlines: &[Inline]) -> String {
        fill(&self.pieces(inlines), usize::MAX, WrapMode::Auto)
    }

    /// Emit a link, keeping the label's internal spaces as wrap points so a long label can break
    /// across lines while the macro's opening (`url[`) and closing (`]`) stay fused to the adjacent
    /// label words.
    fn link(&mut self, inlines: &[Inline], target: &Target, out: &mut Vec<Piece>) {
        let url = &target.url;
        let scheme = url_scheme(url);
        let bare = label_matches_url(&to_plain_text(inlines), url)
            && !url.contains(char::is_whitespace)
            && scheme != Some("mailto");
        if bare {
            out.push(Piece::text(url.to_string()));
            return;
        }
        let prefix = if scheme.is_some_and(is_autolink_scheme) {
            ""
        } else {
            "link:"
        };
        self.wrapped(inlines, &format!("{prefix}{url}["), "]", out);
    }

    fn note(&mut self, blocks: &[Block], out: &mut Vec<Piece>) {
        match blocks {
            [Block::Plain(inlines) | Block::Para(inlines)] => {
                out.push(Piece::text("footnote:["));
                out.extend(self.pieces(inlines));
                out.push(Piece::text("]"));
            }
            _ => out.push(Piece::text("[multiblock footnote omitted]")),
        }
    }
}

/// Put a block anchor line above `body`; an empty identifier leaves the body as it is.
fn anchored(id: &str, body: String) -> String {
    if id.is_empty() {
        body
    } else if body.is_empty() {
        format!("[[{id}]]")
    } else {
        format!("[[{id}]]\n{body}")
    }
}

/// Render a caption's blocks to a title body, or `None` when the caption is empty. Each paragraph is
/// wrapped on its own and successive paragraphs are joined with a line break; the first line is laid
/// out as if one column (the leading `.`) is already consumed.
fn render_caption(caption: &Caption, state: &mut State, width: usize) -> Option<String> {
    if caption.long.is_empty() {
        return None;
    }
    let mut lines = Vec::new();
    for (index, inlines) in crate::common::caption_runs(&caption.long)
        .iter()
        .enumerate()
    {
        let initial = usize::from(index == 0);
        lines.push(fill_offset(
            &state.pieces(inlines),
            width,
            initial,
            state.wrap,
        ));
    }
    let body = lines.join(" +\n");
    (!body.is_empty()).then_some(body)
}

#[cfg(test)]
mod tests;
