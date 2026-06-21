//! `AsciiDoc` writer: renders the document model to `AsciiDoc` markup.
//!
//! Inline content is filled to a 72-column line and block structure is conveyed through the format's
//! line-oriented markup. Output carries no trailing newline; the caller appends one. This format has
//! no public specification, so its rules are stated directly here.

use std::fmt::Write as _;

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, ColWidth, Document, Inline, ListAttributes,
    ListNumberStyle, MathType, QuoteType, Row, Table, TableBody, Target, slug, to_plain_text,
};
use carta_core::{Result, Writer, WriterOptions};

use crate::common::{
    self, FILL_COLUMN, Piece, RawTrim, RowSpanGrid, attribute_value, display_width, fill,
    fill_offset, is_uri_scheme, split_length_unit,
};

/// Renders a document to `AsciiDoc` markup.
#[derive(Debug, Default, Clone, Copy)]
pub struct AsciidocWriter;

impl Writer for AsciidocWriter {
    fn write(&self, document: &Document, _options: &WriterOptions) -> Result<String> {
        let mut state = State::default();
        let body = state.blocks(&document.blocks, FILL_COLUMN);
        Ok(body.trim_end_matches('\n').to_owned())
    }

    fn default_template(&self) -> Option<&'static str> {
        Some(include_str!("templates/default.asciidoc"))
    }

    fn body_ends_with_newline(&self) -> bool {
        true
    }
}

#[derive(Debug, Default)]
struct State {
    bullet_depth: usize,
    ordered_depth: usize,
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
            Block::Plain(inlines) | Block::Para(inlines) => self.paragraph(inlines, width),
            Block::Header(level, attr, inlines) => self.header(*level, attr, inlines),
            Block::CodeBlock(attr, text) => code_block(attr, text),
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
            Block::LineBlock(lines) => self.line_block(lines, width),
        }
    }

    fn paragraph(&mut self, inlines: &[Inline], width: usize) -> String {
        if !inlines
            .iter()
            .any(|inline| matches!(inline, Inline::Math(MathType::DisplayMath, _)))
        {
            return fill(&self.pieces(inlines), width);
        }
        let mut out = String::new();
        let mut text_run: Vec<Inline> = Vec::new();
        let flush_text = |state: &mut Self, run: &mut Vec<Inline>, out: &mut String| {
            if let Some(rendered) = state.text_segment(run, width) {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&rendered);
            }
            run.clear();
        };
        for inline in inlines {
            match inline {
                Inline::Math(MathType::DisplayMath, math) => {
                    flush_text(self, &mut text_run, &mut out);
                    if !out.is_empty() {
                        out.push_str("\n\n");
                    }
                    out.push_str(&display_math_block(math));
                }
                _ => text_run.push(inline.clone()),
            }
        }
        flush_text(self, &mut text_run, &mut out);
        out
    }

    fn text_segment(&mut self, inlines: &[Inline], width: usize) -> Option<String> {
        let trimmed = trim_surrounding_space(inlines);
        if trimmed.is_empty() {
            return None;
        }
        let rendered = fill(&self.pieces(trimmed), width);
        (!rendered.is_empty()).then_some(rendered)
    }

    fn header(&mut self, level: i32, attr: &Attr, inlines: &[Inline]) -> String {
        let depth = usize::try_from(level.max(0)).unwrap_or(0).saturating_add(1);
        let equals = "=".repeat(depth);
        let text = fill(&self.pieces(inlines), FILL_COLUMN);
        let heading = format!("{equals} {text}");
        if !attr.id.is_empty() && attr.id != slug(&to_plain_text(inlines)) {
            format!("[[{}]]\n{heading}", attr.id)
        } else {
            heading
        }
    }

    fn line_block(&mut self, lines: &[Vec<Inline>], width: usize) -> String {
        let body: Vec<String> = lines
            .iter()
            .map(|line| fill(&self.pieces(line), width))
            .collect();
        format!("[verse]\n--\n{}\n--", body.join("\n"))
    }

    fn div(&mut self, attr: &Attr, blocks: &[Block], width: usize) -> String {
        let body = self.blocks(blocks, width);
        let core = match admonition(attr) {
            Some(label) => format!("[{label}]\n====\n{}\n====", body.trim_end_matches('\n')),
            None => body,
        };
        if attr.id.is_empty() {
            core
        } else {
            format!("[[{}]]\n{core}", attr.id)
        }
    }

    fn figure(&mut self, attr: &Attr, caption: &Caption, blocks: &[Block], width: usize) -> String {
        let block_image = match blocks {
            [Block::Plain(inlines) | Block::Para(inlines)] => match inlines.as_slice() {
                [Inline::Image(image_attr, alt, target)] => Some((image_attr, alt, target)),
                _ => None,
            },
            _ => None,
        };
        let core = if let Some((image_attr, alt, target)) = block_image {
            let title_caption = render_caption(caption, self, width);
            let alt_text = self.inline_string(alt);
            let macro_line = format!(
                "image::{}[{}]",
                target.url,
                image_args(image_attr, target, &alt_text)
            );
            match title_caption {
                Some(text) => format!(".{text}\n{macro_line}"),
                None => macro_line,
            }
        } else {
            self.blocks(blocks, width)
        };
        if attr.id.is_empty() {
            core
        } else {
            format!("[[{}]]\n{core}", attr.id)
        }
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
        let mut out = String::new();
        let mut started = false;
        for (index, block) in item.iter().enumerate() {
            let is_list = matches!(block, Block::BulletList(_) | Block::OrderedList(..));
            let rendered = if index == 0 && !is_list {
                let checkbox = task_markers.then(|| task_checkbox(block)).flatten();
                let text = match checkbox {
                    Some((box_marker, rest)) => {
                        let body = self.paragraph(&rest, width);
                        format!("{box_marker} {body}")
                    }
                    None => self.block(block, width),
                };
                format!("{marker} {text}")
            } else {
                self.block(block, width)
            };
            if !started {
                out.push_str(&rendered);
                started = true;
                continue;
            }
            if is_list {
                out.push('\n');
            } else {
                out.push_str("\n+\n");
            }
            out.push_str(&rendered);
        }
        if !started {
            out.push_str(marker);
        }
        out
    }

    fn definition_list(
        &mut self,
        items: &[(Vec<Inline>, Vec<Vec<Block>>)],
        width: usize,
    ) -> String {
        let mut entries = Vec::new();
        for (term, definitions) in items {
            let term_text = fill(&self.pieces(term), width);
            let mut entry = format!("{term_text}::");
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
        entries.join("\n")
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
        let header_rows: Vec<&Row> = table.head.rows.iter().collect();
        let mut out = String::new();
        if let Some(caption) = render_caption(&table.caption, self, width) {
            let _ = writeln!(out, ".{caption}");
        }
        out.push_str(&table_options_line(
            table,
            !header_rows.is_empty(),
            !table.foot.rows.is_empty(),
        ));
        out.push('\n');
        out.push_str("|===");
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
        out.push_str("\n|===");
        out
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
                line.push(' ');
                column = column.saturating_add(1);
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
        if is_simple_cell(&cell.content) {
            let prefix = format!("{marker}|");
            let initial = column.saturating_add(display_width(&prefix));
            let body = self.cell_inlines(&cell.content, width, initial);
            format!("{prefix}{body}")
        } else {
            let body = self.blocks(&cell.content, width);
            format!("{marker}a|\n{}\n", body.trim_end_matches('\n'))
        }
    }

    fn cell_inlines(&mut self, blocks: &[Block], width: usize, initial: usize) -> String {
        match blocks {
            [Block::Plain(inlines) | Block::Para(inlines)] => {
                fill_offset(&self.pieces(inlines), width, initial)
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
            Inline::Emph(inlines) => self.bracketed(inlines, '_', before, after),
            Inline::Strong(inlines) => self.bracketed(inlines, '*', before, after),
            Inline::Strikeout(inlines) => {
                format!("[line-through]#{}#", self.inline_string(inlines))
            }
            Inline::Underline(inlines) => format!("[.underline]#{}#", self.inline_string(inlines)),
            Inline::SmallCaps(inlines) => format!("[smallcaps]#{}#", self.inline_string(inlines)),
            Inline::Superscript(inlines) => format!("^{}^", self.inline_string(inlines)),
            Inline::Subscript(inlines) => format!("~{}~", self.inline_string(inlines)),
            Inline::Quoted(kind, inlines) => {
                let (open, close) = quote_glyphs(kind);
                format!("{open}{}{close}", self.inline_string(inlines))
            }
            Inline::Cite(_, inlines) => {
                for (index, child) in inlines.iter().enumerate() {
                    let child_before = inlines.get(index.wrapping_sub(1)).filter(|_| index > 0);
                    let child_after = inlines.get(index + 1);
                    self.inline(child, child_before, child_after, out);
                }
                return;
            }
            Inline::Code(_, text) => format!("`{}`", escape_text(text)),
            Inline::Space | Inline::SoftBreak => {
                out.push(Piece::Space);
                return;
            }
            Inline::LineBreak => {
                out.push(Piece::Text(" +".to_owned()));
                out.push(Piece::Hard);
                return;
            }
            Inline::Math(_, text) => format!("latexmath:[{text}]"),
            Inline::RawInline(format, text) => {
                let rendered = common::raw_passthrough(format, text, "asciidoc", RawTrim::DropOne);
                if !rendered.is_empty() {
                    out.push(Piece::Text(rendered));
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
            Inline::Span(attr, inlines) => self.span(attr, inlines),
            Inline::Note(blocks) => self.note(blocks),
        };
        out.push(Piece::Text(text));
    }

    /// Render an inline sequence to a single string with no line filling: spaces stay literal.
    fn inline_string(&mut self, inlines: &[Inline]) -> String {
        fill(&self.pieces(inlines), usize::MAX)
    }

    /// Render emphasis or strong with its single-character marker, choosing the unconstrained
    /// (doubled marker) form when the construct abuts a word character on either side.
    fn bracketed(
        &mut self,
        inlines: &[Inline],
        marker: char,
        before: Option<&Inline>,
        after: Option<&Inline>,
    ) -> String {
        let body = self.inline_string(inlines);
        let unconstrained = closes_left_boundary(before) || closes_right_boundary(after);
        if unconstrained {
            format!("{marker}{marker}{body}{marker}{marker}")
        } else {
            format!("{marker}{body}{marker}")
        }
    }

    /// Emit a link, keeping the label's internal spaces as wrap points so a long label can break
    /// across lines while the macro's opening (`url[`) and closing (`]`) stay fused to the adjacent
    /// label words.
    fn link(&mut self, inlines: &[Inline], target: &Target, out: &mut Vec<Piece>) {
        let url = &target.url;
        let scheme = url_scheme(url);
        let bare = to_plain_text(inlines) == *url
            && !url.contains(char::is_whitespace)
            && scheme != Some("mailto");
        if bare {
            out.push(Piece::Text(url.clone()));
            return;
        }
        let prefix = if scheme.is_some_and(is_autolink_scheme) {
            ""
        } else {
            "link:"
        };
        let opening = format!("{prefix}{url}[");
        let mut label = self.pieces(inlines);
        let first_text = label
            .iter()
            .position(|piece| matches!(piece, Piece::Text(_)));
        match first_text {
            Some(index) => {
                if let Some(Piece::Text(text)) = label.get_mut(index) {
                    *text = format!("{opening}{text}");
                }
                if let Some(Piece::Text(text)) = label
                    .iter_mut()
                    .rev()
                    .find(|piece| matches!(piece, Piece::Text(_)))
                {
                    text.push(']');
                }
                out.append(&mut label);
            }
            None => out.push(Piece::Text(format!("{opening}]"))),
        }
    }

    fn span(&mut self, attr: &Attr, inlines: &[Inline]) -> String {
        let body = self.inline_string(inlines);
        match span_role(attr) {
            Some(role) => format!("[{role}]#{body}#"),
            None => body,
        }
    }

    fn note(&mut self, blocks: &[Block]) -> String {
        match blocks {
            [Block::Plain(inlines) | Block::Para(inlines)] => {
                format!("footnote:[{}]", self.inline_string(inlines))
            }
            _ => "[multiblock footnote omitted]".to_owned(),
        }
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
    for (index, block) in caption.long.iter().enumerate() {
        if let Block::Plain(inlines) | Block::Para(inlines) = block {
            let initial = usize::from(index == 0);
            lines.push(fill_offset(&state.pieces(inlines), width, initial));
        }
    }
    let body = lines.join(" +\n");
    (!body.is_empty()).then_some(body)
}

/// The URI scheme of a URL: the lowercase run before the first `:`, when that run is a valid scheme
/// (a letter followed by letters, digits, `+`, `-`, or `.`).
fn url_scheme(url: &str) -> Option<&str> {
    let colon = url.find(':')?;
    let scheme = url.get(..colon)?;
    is_uri_scheme(scheme).then_some(scheme)
}

/// Whether a scheme is one the format auto-recognizes as a link, so its URL needs no `link:` prefix.
fn is_autolink_scheme(scheme: &str) -> bool {
    ["http", "https", "ftp", "irc", "mailto"]
        .iter()
        .any(|known| scheme.eq_ignore_ascii_case(known))
}

/// The `image:`/`image::` argument list: the alt text (the URL's file stem when no alt is given),
/// the title, and a width/height descriptor.
fn image_args(attr: &Attr, target: &Target, alt: &str) -> String {
    let alt = if alt.is_empty() {
        image_stem(&target.url)
    } else {
        alt.to_owned()
    };
    let mut parts = vec![alt];
    if !target.title.is_empty() {
        parts.push(format!("title=\"{}\"", target.title));
    }
    if let Some(size) = image_size(attr) {
        parts.push(size);
    }
    parts.join(",")
}

/// The default alt text for an image with none of its own: the file name of the target URL, minus
/// any directory and extension.
fn image_stem(url: &str) -> String {
    let file = url.rsplit(['/', '\\']).next().unwrap_or(url);
    let stem = file.rsplit_once('.').map_or(file, |(name, _)| name);
    stem.to_owned()
}

/// An image's size descriptor: a percentage width becomes `scaledwidth`, an absolute width or height
/// becomes `width`/`height` with any unit suffix stripped.
fn image_size(attr: &Attr) -> Option<String> {
    if let Some(width) = attribute_value(attr, "width") {
        if let Some(value) = width
            .strip_suffix('%')
            .and_then(|percent| percent.parse::<f64>().ok())
        {
            return Some(format!("scaledwidth={}%", format_decimal(value)));
        }
        return Some(format!("width={}", split_length_unit(width).0));
    }
    if let Some(height) = attribute_value(attr, "height") {
        return Some(format!("height={}", split_length_unit(height).0));
    }
    None
}

/// Format a number the way the format's scaled-width values appear: an integer keeps a single trailing
/// zero (`50` -> `50.0`), a fractional value keeps its digits (`50.5`).
fn format_decimal(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        format!("{value}")
    }
}

/// The bracketed role for a span: its id (`#id`) and classes (`.class`), space-joined. `None` when
/// the span carries neither.
fn span_role(attr: &Attr) -> Option<String> {
    let mut parts = Vec::new();
    if !attr.id.is_empty() {
        parts.push(format!("#{}", attr.id));
    }
    for class in &attr.classes {
        parts.push(format!(".{class}"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

/// The admonition label for a div whose classes name one, uppercased.
fn admonition(attr: &Attr) -> Option<&'static str> {
    attr.classes.iter().find_map(|class| match class.as_str() {
        "note" => Some("NOTE"),
        "tip" => Some("TIP"),
        "important" => Some("IMPORTANT"),
        "caution" => Some("CAUTION"),
        "warning" => Some("WARNING"),
        _ => None,
    })
}

/// The opening character for an emphasis/strong construct must be the unconstrained (doubled) form
/// when the preceding inline closes the left word boundary: a string ending in an alphanumeric, or
/// any non-space formatted inline.
fn closes_left_boundary(before: Option<&Inline>) -> bool {
    match before {
        None | Some(Inline::Space | Inline::SoftBreak | Inline::LineBreak) => false,
        Some(Inline::Str(text)) => text.chars().last().is_some_and(char::is_alphanumeric),
        Some(_) => true,
    }
}

fn closes_right_boundary(after: Option<&Inline>) -> bool {
    match after {
        None | Some(Inline::Space | Inline::SoftBreak | Inline::LineBreak) => false,
        Some(Inline::Str(text)) => text.chars().next().is_some_and(char::is_alphanumeric),
        Some(_) => true,
    }
}

/// The smart-quote glyphs wrapped in the format's typographic-quote backticks.
fn quote_glyphs(kind: &QuoteType) -> (String, String) {
    match kind {
        QuoteType::SingleQuote => ("'`".to_owned(), "`'".to_owned()),
        QuoteType::DoubleQuote => ("\"`".to_owned(), "`\"".to_owned()),
    }
}

/// The attribute line preceding an ordered list's items, naming its numeral style and (when not one)
/// its start number. An example-numbered list takes no attribute line.
fn ordered_style_line(attrs: &ListAttributes) -> Option<String> {
    let style = match attrs.style {
        ListNumberStyle::Example => return None,
        ListNumberStyle::DefaultStyle | ListNumberStyle::Decimal => "arabic",
        ListNumberStyle::LowerAlpha => "loweralpha",
        ListNumberStyle::UpperAlpha => "upperalpha",
        ListNumberStyle::LowerRoman => "lowerroman",
        ListNumberStyle::UpperRoman => "upperroman",
    };
    if attrs.start == 1 {
        Some(format!("[{style}]"))
    } else {
        Some(format!("[{style}, start={}]", attrs.start))
    }
}

/// The `[…]` options line introducing a table: an overall width percentage, per-column widths, and
/// a `header` option when the table has a header row.
fn table_options_line(table: &Table, has_header: bool, has_footer: bool) -> String {
    let widths: Vec<f64> = table
        .col_specs
        .iter()
        .map(|spec| match spec.width {
            ColWidth::ColWidth(value) => value,
            ColWidth::ColWidthDefault => 0.0,
        })
        .collect();
    let total: f64 = widths.iter().sum();
    let mut attrs = Vec::new();
    if total > 0.0 {
        attrs.push(format!("width=\"{}%\"", percent_truncated(total)));
    }
    let cols: Vec<String> = table
        .col_specs
        .iter()
        .zip(&widths)
        .map(|(spec, width)| {
            let operator = alignment_operator(&spec.align)
                .map(|op| op.to_string())
                .unwrap_or_default();
            if total > 0.0 && *width > 0.0 {
                format!("{operator}{}%", percent_rounded(width / total))
            } else {
                operator
            }
        })
        .collect();
    attrs.push(format!("cols=\"{}\"", cols.join(",")));
    let mut options = Vec::new();
    if has_header {
        options.push("header");
    }
    if has_footer {
        options.push("footer");
    }
    if !options.is_empty() {
        attrs.push(format!("options=\"{}\"", options.join(",")));
    }
    format!("[{},]", attrs.join(","))
}

/// Detect a leading task-list checkbox on a list item's first block. Returns the literal marker
/// (`[ ]` or `[x]`) and the remaining inlines with the checkbox glyph and its trailing space
/// removed.
fn task_checkbox(block: &Block) -> Option<(&'static str, Vec<Inline>)> {
    let (Block::Plain(inlines) | Block::Para(inlines)) = block else {
        return None;
    };
    let marker = match inlines.first() {
        Some(Inline::Str(glyph)) if glyph == "\u{2610}" => "[ ]",
        Some(Inline::Str(glyph)) if glyph == "\u{2612}" => "[x]",
        _ => return None,
    };
    match inlines.get(1) {
        Some(Inline::Space) => Some((marker, inlines.get(2..).unwrap_or(&[]).to_vec())),
        _ => None,
    }
}

/// A display equation rendered as its own delimited block.
fn display_math_block(math: &str) -> String {
    format!("[latexmath]\n++++\n{math}\n++++")
}

/// Drop leading and trailing whitespace inlines from a run, since a block boundary already supplies
/// the separation.
fn trim_surrounding_space(inlines: &[Inline]) -> &[Inline] {
    let is_space = |inline: &Inline| matches!(inline, Inline::Space | Inline::SoftBreak);
    let start = inlines.iter().position(|inline| !is_space(inline));
    match start {
        Some(start) => {
            let end = inlines
                .iter()
                .rposition(|inline| !is_space(inline))
                .unwrap_or(start);
            inlines.get(start..=end).unwrap_or(&[])
        }
        None => &[],
    }
}

/// A fraction in `0.0..=1.0` as a whole-number percentage string, truncated toward zero.
fn percent_truncated(fraction: f64) -> String {
    format!("{:.0}", (fraction * 100.0).floor().max(0.0))
}

/// A fraction in `0.0..=1.0` as a whole-number percentage string, rounded to nearest.
fn percent_rounded(fraction: f64) -> String {
    format!("{:.0}", (fraction * 100.0).round().max(0.0))
}

/// The span prefix for a table cell: a colspan, a rowspan (`.n`), or both (`c.r`). Empty when the
/// cell spans a single column and row.
fn cell_span(cell: &Cell) -> String {
    let col = (cell.col_span > 1).then(|| cell.col_span.to_string());
    let row = (cell.row_span > 1).then(|| format!(".{}", cell.row_span));
    match (col, row) {
        (Some(c), Some(r)) => format!("{c}{r}"),
        (Some(c), None) => c,
        (None, Some(r)) => r,
        (None, None) => String::new(),
    }
}

fn alignment_operator(align: &Alignment) -> Option<char> {
    match align {
        Alignment::AlignLeft => Some('<'),
        Alignment::AlignCenter => Some('^'),
        Alignment::AlignRight => Some('>'),
        Alignment::AlignDefault => None,
    }
}

/// Whether a cell's content is a single text block, so it renders on the marker line rather than as
/// a block cell.
fn is_simple_cell(blocks: &[Block]) -> bool {
    matches!(blocks, [] | [Block::Plain(_) | Block::Para(_)])
}

/// Render a code block: a `[source,…]` delimited block when the block carries classes (with a
/// `%linesnum` flag for `numberLines`), otherwise a literal `....` block.
fn code_block(attr: &Attr, text: &str) -> String {
    let body = text.strip_suffix('\n').unwrap_or(text);
    if attr.classes.is_empty() {
        return format!("....\n{body}\n....");
    }
    let numbered = attr.classes.iter().any(|class| class == "numberLines");
    let languages: Vec<&str> = attr
        .classes
        .iter()
        .filter(|class| class.as_str() != "numberLines")
        .map(String::as_str)
        .collect();
    let mut header = String::from("[source");
    if numbered {
        header.push_str("%linesnum");
    }
    if !languages.is_empty() {
        let _ = write!(header, ",{}", languages.join(","));
    }
    header.push(']');
    format!("{header}\n----\n{body}\n----")
}

/// Escape a run of plain text. A maximal run of formatting characters is wrapped together in a
/// single passthrough span (`++…++`); `+` is always replaced by its attribute reference since it
/// would otherwise begin a passthrough span itself.
fn escape_text(text: &str) -> String {
    let mut out = String::new();
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if !run.is_empty() {
            let _ = write!(out, "++{run}++");
            run.clear();
        }
    };
    for ch in text.chars() {
        if is_formatting_char(ch) {
            run.push(ch);
        } else {
            flush(&mut run, &mut out);
            if ch == '+' {
                out.push_str("{plus}");
            } else {
                out.push(ch);
            }
        }
    }
    flush(&mut run, &mut out);
    out
}

/// Whether a character begins or participates in inline formatting and so must be passed through
/// literally. `}` is left alone: it is significant only paired with `{`, which is itself escaped.
fn is_formatting_char(ch: char) -> bool {
    matches!(
        ch,
        '*' | '_' | '`' | '#' | '<' | '>' | '{' | '[' | ']' | '|' | '\\'
    )
}

/// Indent every non-empty line of a body by a fixed prefix; blank separator lines stay empty.
fn indent(body: &str, prefix: &str) -> String {
    let mut out = String::new();
    for (index, line) in body.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        if line.is_empty() {
            continue;
        }
        out.push_str(prefix);
        out.push_str(line);
    }
    out
}
