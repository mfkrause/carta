//! Plain-text writer: renders the document model to unformatted text.
//!
//! Output uses a fill column of 72, strips inline markup (emphasis, links, and inline code render as
//! their textual content), and conveys block structure through indentation alone. It carries no
//! trailing newline; the caller appends one. This format has no public specification.

use carta_ast::{
    Alignment, Attr, Block, ColWidth, Document, Format, Inline, ListAttributes, MathType,
    QuoteType, Row, Table,
};
use carta_core::{Extension, Result, WrapMode, Writer, WriterOptions};

use crate::common::{
    FILL_COLUMN, MEASURE_WIDTH, NotesHost, Piece, TableForm, append_notes, ascii_punctuation,
    block_inlines, body_rows, cell_inlines, dash_rule, display_width, extend_multiline_body, fill,
    fill_hang, fill_offset, filled_cells, indent_block, indent_lines, is_loose, is_uri,
    item_separator, join_loose, label_matches_url, lay_row, measure_pieces, offset_as_i32,
    ordered_marker, pad_marker, pieces_nonempty, quote_marks, table_form,
};
use crate::grid;

/// Renders a document to plain text.
#[derive(Debug, Default, Clone, Copy)]
pub struct PlainWriter;

impl Writer for PlainWriter {
    fn write(&self, document: &Document, options: &WriterOptions) -> Result<String> {
        let width = options.columns.unwrap_or(FILL_COLUMN);
        let mut state = State {
            wrap: options.wrap,
            width,
            smart: options.extensions.contains(Extension::Smart),
            ..State::default()
        };
        let body = state.blocks_to_string(&document.blocks, width, false);
        Ok(append_notes(body, &state.footnotes))
    }

    fn default_template(&self) -> Option<&'static str> {
        Some(include_str!("templates/default.plain"))
    }

    fn body_ends_with_newline(&self) -> bool {
        true
    }
}

/// Carries the footnote bodies accumulated while rendering, so notes can be collected inline and
/// emitted as a section at the end of the document, along with the configured fill width.
#[derive(Debug)]
struct State {
    footnotes: Vec<String>,
    wrap: WrapMode,
    width: usize,
    /// Whether `smart` punctuation is rendered: quotes become straight ASCII and Unicode dashes and
    /// the ellipsis collapse to their ASCII forms, rather than passing through as literal Unicode.
    smart: bool,
    /// How many tables the current render is nested inside, counting the one being rendered.
    table_depth: usize,
}

impl Default for State {
    fn default() -> Self {
        Self {
            footnotes: Vec::new(),
            wrap: WrapMode::default(),
            width: FILL_COLUMN,
            smart: false,
            table_depth: 0,
        }
    }
}

impl State {
    /// Render a block sequence with a blank line between blocks, dropping those that produce no
    /// output. This is the default layout (document body, block quotes, divs, figures, loose list
    /// items, loose definitions). See [`join_loose`] for the [`Block::Plain`] spacing quirk. When
    /// `hang` is set the first non-empty block keeps a space that opens it, so content laid out under
    /// a list marker or block-quote indent keeps the gap the source put after that prefix.
    fn blocks_to_string(&mut self, blocks: &[Block], width: usize, hang: bool) -> String {
        let mut rendered = Vec::with_capacity(blocks.len());
        let mut first = true;
        for block in blocks {
            let text = self.block(block, width, hang && first);
            if !text.is_empty() {
                first = false;
            }
            rendered.push((matches!(block, Block::Plain(_)), text));
        }
        join_loose(rendered)
    }

    /// Render a block sequence with a single newline between blocks: the compact layout used inside a
    /// tight list's items and tight definitions. `hang` behaves as in [`Self::blocks_to_string`].
    fn blocks_tight(&mut self, blocks: &[Block], width: usize, hang: bool) -> String {
        let mut parts: Vec<String> = Vec::new();
        let mut first = true;
        for block in blocks {
            let text = self.block(block, width, hang && first);
            if text.is_empty() {
                continue;
            }
            first = false;
            parts.push(text);
        }
        parts.join("\n")
    }

    /// Render a block sequence at the given layout density.
    fn blocks_at(&mut self, blocks: &[Block], width: usize, loose: bool, hang: bool) -> String {
        if loose {
            self.blocks_to_string(blocks, width, hang)
        } else {
            self.blocks_tight(blocks, width, hang)
        }
    }

    fn block(&mut self, block: &Block, width: usize, hang: bool) -> String {
        match block {
            Block::Plain(inlines) | Block::Para(inlines) => {
                let pieces = self.pieces(inlines);
                if hang {
                    fill_hang(&pieces, width, self.wrap)
                } else {
                    fill(&pieces, width, self.wrap)
                }
            }
            Block::Header(_, _, inlines) => {
                let pieces = self.pieces(inlines);
                header_text(&pieces)
            }
            Block::CodeBlock(_, text) => {
                indent_block(text.strip_suffix('\n').unwrap_or(text), "    ", "    ")
            }
            Block::RawBlock(format, text) => {
                if is_plain_format(format) {
                    text.strip_suffix('\n').unwrap_or(text).to_owned()
                } else {
                    String::new()
                }
            }
            Block::BlockQuote(blocks) => {
                let body = self.blocks_to_string(blocks, width.saturating_sub(2), true);
                indent_block(&body, "  ", "  ")
            }
            Block::BulletList(items) => self.bullet_list(items, width),
            Block::OrderedList(attrs, items) => self.ordered_list(attrs, items, width),
            Block::DefinitionList(items) => self.definition_list(items, width),
            Block::HorizontalRule => "-".repeat(width),
            Block::Table(table) => self.table(table, width),
            Block::Figure(attr, caption, blocks) => {
                match self.simple_figure(attr, &caption.long, blocks) {
                    Some(rendered) => rendered,
                    None => self.blocks_to_string(blocks, width, false),
                }
            }
            Block::Div(_, blocks) => self.blocks_to_string(blocks, width, false),
            Block::LineBlock(lines) => self.line_block(lines),
        }
    }

    /// A figure with no attributes whose body is a single bare image (a `Plain` holding one image
    /// that itself carries no identifier or classes) renders as that image with the caption text in
    /// place of its alternate text: `[caption]`. Any richer figure returns `None` so the caller
    /// renders the body blocks instead.
    fn simple_figure(
        &mut self,
        attr: &Attr,
        caption_long: &[Block],
        blocks: &[Block],
    ) -> Option<String> {
        if !attr.id.is_empty() || !attr.classes.is_empty() || !attr.attributes.is_empty() {
            return None;
        }
        let [Block::Plain(inlines)] = blocks else {
            return None;
        };
        let [Inline::Image(image_attr, _, target)] = inlines.as_slice() else {
            return None;
        };
        if !image_attr.id.is_empty()
            || !image_attr.classes.is_empty()
            || !image_attr.attributes.is_empty()
        {
            return None;
        }
        let caption_inlines = figure_caption_inlines(caption_long)?;
        let image = Inline::Image(image_attr.clone(), caption_inlines, target.clone());
        let mut out = Vec::new();
        self.inline(&image, &mut out);
        Some(pieces_to_string(&out))
    }

    fn line_block(&mut self, lines: &[Vec<Inline>]) -> String {
        let rendered: Vec<String> = lines
            .iter()
            .map(|line| {
                let pieces = self.pieces(line);
                pieces_to_string(&pieces)
            })
            .collect();
        rendered.join("\n")
    }

    fn bullet_list(&mut self, items: &[Vec<Block>], width: usize) -> String {
        let loose = is_loose(items);
        let body_width = width.saturating_sub(2);
        let rendered: Vec<String> = items
            .iter()
            .map(|item| {
                let body = self.blocks_at(item, body_width, loose, true);
                indent_block(&body, "- ", "  ")
            })
            .collect();
        rendered.join(item_separator(loose))
    }

    fn ordered_list(
        &mut self,
        attrs: &ListAttributes,
        items: &[Vec<Block>],
        width: usize,
    ) -> String {
        let loose = is_loose(items);
        let rendered: Vec<String> = items
            .iter()
            .enumerate()
            .map(|(offset, item)| {
                let number = attrs.start.saturating_add(offset_as_i32(offset));
                let marker = ordered_marker(number, attrs.style, attrs.delim);
                let field = (marker.chars().count() + 1).max(4);
                let body = self.blocks_at(item, width.saturating_sub(field), loose, true);
                let first = pad_marker(&marker, field);
                let rest = " ".repeat(field);
                indent_block(&body, &first, &rest)
            })
            .collect();
        rendered.join(item_separator(loose))
    }

    fn definition_list(
        &mut self,
        items: &[(Vec<Inline>, Vec<Vec<Block>>)],
        width: usize,
    ) -> String {
        let groups: Vec<String> = items
            .iter()
            .map(|(term, definitions)| {
                let term_pieces = self.pieces(term);
                let mut group = fill(&term_pieces, width, self.wrap);
                for definition in definitions {
                    let loose = is_loose_definition(definition);
                    let body = self.blocks_at(definition, width.saturating_sub(2), loose, true);
                    let indented = indent_block(&body, "  ", "  ");
                    // An empty term contributes no line, so the first body opens the group with no
                    // leading separator; otherwise each body is set off from what precedes it.
                    if !group.is_empty() {
                        group.push_str(if loose { "\n\n" } else { "\n" });
                    }
                    group.push_str(&indented);
                }
                group
            })
            .collect();
        groups.join("\n\n")
    }

    fn table(&mut self, table: &Table, width: usize) -> String {
        if table.col_specs.is_empty() {
            return String::new();
        }
        let form = table_form(table);
        self.table_depth += 1;
        let body = match form {
            TableForm::Simple => self.simple_table(table),
            TableForm::Multiline => self.multiline_table(table, width),
            TableForm::Grid => self.grid_table(table, width),
        };
        self.table_depth = self.table_depth.saturating_sub(1);
        match self.table_caption(table, form, width) {
            Some(caption) if body.is_empty() => caption,
            Some(caption) => format!("{body}\n\n{caption}"),
            None => body,
        }
    }

    /// A simple table: one line per cell, column width sized to the widest cell. A non-empty header
    /// is underlined with a per-column dash rule; a headerless table is fenced by dash rules above
    /// and below. Indented two columns.
    fn simple_table(&mut self, table: &Table) -> String {
        let columns = table.col_specs.len();
        let aligns: Vec<&Alignment> = table.col_specs.iter().map(|spec| &spec.align).collect();
        let header: Vec<Vec<String>> = table
            .head
            .rows
            .iter()
            .map(|row| self.simple_row(row, columns))
            .collect();
        let body: Vec<Vec<String>> = body_rows(table)
            .iter()
            .map(|row| self.simple_row(row, columns))
            .collect();
        let has_header = header
            .iter()
            .any(|row| row.iter().any(|text| !text.is_empty()));

        let mut field = vec![0usize; columns];
        for row in header.iter().chain(body.iter()) {
            for (index, text) in row.iter().enumerate() {
                if let Some(width) = field.get_mut(index) {
                    *width = (*width).max(display_width(text) + 2);
                }
            }
        }
        let rule = dash_rule(&field);

        let mut lines: Vec<String> = Vec::new();
        let lay = |row: &[String]| {
            let cells: Vec<Vec<String>> = row.iter().map(|text| vec![text.clone()]).collect();
            lay_row(&cells, &field, &aligns)
        };
        if has_header {
            for row in &header {
                lines.extend(lay(row));
            }
            lines.push(rule);
            for row in &body {
                lines.extend(lay(row));
            }
        } else {
            lines.push(rule.clone());
            for row in &body {
                lines.extend(lay(row));
            }
            lines.push(rule);
        }
        indent_lines(&lines, 2)
    }

    /// A multiline table: cells wrap within their column and rows are separated by blank lines.
    /// Column widths come from explicit fractional specs (floored at the widest unbreakable word)
    /// or, lacking those, from the natural content width. Indented two columns.
    fn multiline_table(&mut self, table: &Table, width: usize) -> String {
        let columns = table.col_specs.len();
        let aligns: Vec<&Alignment> = table.col_specs.iter().map(|spec| &spec.align).collect();
        let header: Vec<Vec<Vec<Piece>>> = table
            .head
            .rows
            .iter()
            .map(|row| self.row_pieces(row, columns))
            .collect();
        let body: Vec<Vec<Vec<Piece>>> = body_rows(table)
            .iter()
            .map(|row| self.row_pieces(row, columns))
            .collect();
        let has_header = header
            .iter()
            .any(|row| row.iter().any(|cell| pieces_nonempty(cell)));

        let mut natural = vec![0usize; columns];
        let mut minword = vec![0usize; columns];
        for row in header.iter().chain(body.iter()) {
            for (index, cell) in row.iter().enumerate() {
                let (cell_width, word) = measure_pieces(cell);
                if let Some(value) = natural.get_mut(index) {
                    *value = (*value).max(cell_width);
                }
                if let Some(value) = minword.get_mut(index) {
                    *value = (*value).max(word);
                }
            }
        }
        let field: Vec<usize> = (0..columns)
            .map(
                |index| match table.col_specs.get(index).map(|spec| &spec.width) {
                    Some(ColWidth::ColWidth(fraction)) if *fraction > 0.0 => {
                        // `floor`/`max(0.0)` make the fraction-to-width conversion exact and
                        // non-negative; the final `min(width)` clamps a spec whose fraction exceeds
                        // the whole line — a meaningless width that would otherwise allocate a rule
                        // of that many characters.
                        #[allow(
                            clippy::cast_precision_loss,
                            clippy::cast_possible_truncation,
                            clippy::cast_sign_loss
                        )]
                        let scaled = (fraction * width.saturating_sub(1) as f64)
                            .floor()
                            .max(0.0)
                            .min(width as f64) as usize;
                        scaled.max(minword.get(index).copied().unwrap_or(0) + 2)
                    }
                    _ => natural.get(index).copied().unwrap_or(0) + 2,
                },
            )
            .collect();

        let contiguous = "-".repeat(field.iter().sum::<usize>() + columns.saturating_sub(1));
        let percolumn = dash_rule(&field);
        let mut lines: Vec<String> = Vec::new();
        if has_header {
            lines.push(contiguous.clone());
            for row in &header {
                lines.extend(lay_row(&filled_cells(row, &field), &field, &aligns));
            }
            lines.push(percolumn);
            extend_multiline_body(&mut lines, &body, &field, &aligns);
            lines.push(contiguous);
        } else {
            lines.push(percolumn.clone());
            extend_multiline_body(&mut lines, &body, &field, &aligns);
            lines.push(percolumn);
        }
        indent_lines(&lines, 2)
    }

    /// A grid table: bordered cells that carry spans, block-level content, or a footer. Column
    /// widths come from explicit fractional specs or a content-proportional fit; the engine in
    /// [`crate::grid`] draws the borders. Not indented.
    fn grid_table(&mut self, table: &Table, width: usize) -> String {
        let columns = table.col_specs.len();
        let aligns: Vec<Alignment> = table
            .col_specs
            .iter()
            .map(|spec| spec.align.clone())
            .collect();
        let head: Vec<&Row> = table.head.rows.iter().collect();
        let body = body_rows(table);
        let foot: Vec<&Row> = table.foot.rows.iter().collect();
        let head_layout = grid::place_columns(&head, columns);
        let body_layout = grid::place_columns(&body, columns);
        let foot_layout = grid::place_columns(&foot, columns);

        let mut natural = vec![0usize; columns];
        let mut minword = vec![0usize; columns];
        if self.table_depth > grid::MAX_MEASURED_TABLE_NESTING {
            // Sizing a column renders every cell a second time, so with nested tables the
            // measurement passes compound into one full render per ancestor. Past the nesting cap,
            // columns take an even share of the fill width instead of being measured, keeping the
            // total work linear in the document.
            let share = (width / columns.max(1)).max(1);
            natural.fill(share);
            minword.fill(1);
        } else {
            let snapshot = self.footnotes.len();
            for (rows, layout) in [
                (&head, &head_layout),
                (&body, &body_layout),
                (&foot, &foot_layout),
            ] {
                self.measure_grid(rows, layout, &mut natural, &mut minword);
            }
            self.footnotes.truncate(snapshot);
        }

        let colspans: Vec<(usize, usize)> = [&head_layout, &body_layout, &foot_layout]
            .into_iter()
            .flatten()
            .flatten()
            .copied()
            .filter(|&(_, span)| span > 1)
            .collect();
        let content = grid::grid_content_widths(
            &table.col_specs,
            &natural,
            &minword,
            &colspans,
            columns,
            width,
            self.wrap,
        );
        let col_widths: Vec<usize> = content
            .iter()
            .map(|content_width| content_width + 2)
            .collect();
        let head_grid = self.grid_rows(&head, &head_layout, &content);
        let body_grid = self.grid_rows(&body, &body_layout, &content);
        let foot_grid = self.grid_rows(&foot, &foot_layout, &content);

        grid::render(&grid::GridTable {
            col_widths,
            aligns: Some(aligns.as_slice()),
            head: head_grid,
            body: body_grid,
            foot: foot_grid,
        })
    }

    /// Render a row's cells to single lines, one per column, padding a short row with empty cells.
    fn simple_row(&mut self, row: &Row, columns: usize) -> Vec<String> {
        let mut out = vec![String::new(); columns];
        for (index, cell) in row.cells.iter().enumerate() {
            if let Some(slot) = out.get_mut(index) {
                let pieces = self.pieces(cell_inlines(cell));
                join_pieces(&pieces, ' ')
                    .trim_start_matches(' ')
                    .clone_into(slot);
            }
        }
        out
    }

    /// Render a row's cells to inline pieces, one entry per column, padding a short row with empty
    /// cells. Building the pieces once records any footnotes a single time.
    fn row_pieces(&mut self, row: &Row, columns: usize) -> Vec<Vec<Piece>> {
        let mut out: Vec<Vec<Piece>> = (0..columns).map(|_| Vec::new()).collect();
        for (index, cell) in row.cells.iter().enumerate() {
            if let Some(slot) = out.get_mut(index) {
                *slot = self.pieces(cell_inlines(cell));
            }
        }
        out
    }

    /// Accumulate the natural and longest-word widths of every single-column cell into the
    /// per-column maxima, rendering each cell at an unconstrained width.
    fn measure_grid(
        &mut self,
        rows: &[&Row],
        layout: &[Vec<(usize, usize)>],
        natural: &mut [usize],
        minword: &mut [usize],
    ) {
        for (row_index, row) in rows.iter().enumerate() {
            for (cell_index, cell) in row.cells.iter().enumerate() {
                let Some(&(start, span)) = layout
                    .get(row_index)
                    .and_then(|placements| placements.get(cell_index))
                else {
                    continue;
                };
                let lines = self.cell_lines(&cell.content, MEASURE_WIDTH);
                let (width, word) = grid::measure_lines(&lines);
                let share_natural = width.div_ceil(span.max(1));
                let share_word = word.div_ceil(span.max(1));
                for column in start..start + span {
                    if let Some(value) = natural.get_mut(column) {
                        *value = (*value).max(share_natural);
                    }
                    if let Some(value) = minword.get_mut(column) {
                        *value = (*value).max(share_word);
                    }
                }
            }
        }
    }

    /// Build the grid rows for one section, rendering each cell's content to lines at the width of
    /// the columns it spans.
    fn grid_rows(
        &mut self,
        rows: &[&Row],
        layout: &[Vec<(usize, usize)>],
        content: &[usize],
    ) -> Vec<grid::GridRow> {
        let mut result = Vec::with_capacity(rows.len());
        for (row_index, row) in rows.iter().enumerate() {
            let mut cells = Vec::with_capacity(row.cells.len());
            for (cell_index, cell) in row.cells.iter().enumerate() {
                let Some(&(start, span)) = layout
                    .get(row_index)
                    .and_then(|placements| placements.get(cell_index))
                else {
                    continue;
                };
                let width = grid::merged_width(content, start, span);
                let lines = self.cell_lines(&cell.content, width);
                cells.push(grid::GridCell {
                    lines,
                    row_span: grid::span_count(cell.row_span),
                    col_span: grid::span_count(cell.col_span),
                });
            }
            result.push(grid::GridRow { cells });
        }
        result
    }

    /// Render a cell's block content to lines at the given width.
    fn cell_lines(&mut self, content: &[Block], width: usize) -> Vec<String> {
        let text = self.blocks_to_string(content, width, false);
        if text.is_empty() {
            Vec::new()
        } else {
            text.split('\n').map(str::to_owned).collect()
        }
    }

    /// The caption block, prefixed `: ` and indented to match the table form (two columns for
    /// simple and multiline tables, none for grids). A non-empty caption carries any table
    /// attributes as a trailing `{#id .class key="value"}` suffix.
    fn table_caption(&mut self, table: &Table, form: TableForm, width: usize) -> Option<String> {
        let base = if matches!(form, TableForm::Grid) {
            0
        } else {
            2
        };
        let mut pieces: Vec<Piece> = Vec::new();
        for block in &table.caption.long {
            if !pieces.is_empty() {
                pieces.push(Piece::Hard);
            }
            self.extend_pieces(block_inlines(block), &mut pieces);
        }
        if !pieces_nonempty(&pieces) {
            return None;
        }
        if let Some(suffix) = attribute_suffix(&table.attr) {
            pieces.push(Piece::Space);
            pieces.push(Piece::text(suffix));
        }
        let body = fill_offset(&pieces, width.saturating_sub(base), 2, self.wrap);
        let first = format!("{}: ", " ".repeat(base));
        let rest = " ".repeat(base);
        Some(indent_block(&body, &first, &rest))
    }

    fn pieces(&mut self, inlines: &[Inline]) -> Vec<Piece> {
        let mut out = Vec::new();
        self.extend_pieces(inlines, &mut out);
        out
    }

    /// Append the inline sequence's pieces to `out`. A `Str` ending in `!` immediately before a link
    /// or span is escaped so it is not re-read as the image marker.
    fn extend_pieces(&mut self, inlines: &[Inline], out: &mut Vec<Piece>) {
        for (position, inline) in inlines.iter().enumerate() {
            if let Inline::Str(text) = inline
                && let Some(prefix) = text.strip_suffix('!')
                && matches!(
                    inlines.get(position + 1),
                    Some(Inline::Link(..) | Inline::Span(..))
                )
            {
                out.push(Piece::text(format!("{prefix}\\!")));
                continue;
            }
            self.inline(inline, out);
        }
    }

    fn inline(&mut self, inline: &Inline, out: &mut Vec<Piece>) {
        match inline {
            Inline::Str(text) => out.push(Piece::text(if self.smart {
                unsmarten(text)
            } else {
                text.to_string()
            })),
            Inline::Code(_, text) => out.push(Piece::text(text.to_string())),
            Inline::Emph(inlines)
            | Inline::Strong(inlines)
            | Inline::Underline(inlines)
            | Inline::Cite(_, inlines)
            | Inline::Span(_, inlines) => self.extend_pieces(inlines, out),
            // A bare URL — its single-`Str` text being the visible form of the target — is the
            // address itself, encoded; any other link contributes only its visible text.
            Inline::Link(_, inlines, target) => {
                if let [Inline::Str(text)] = inlines.as_slice()
                    && is_uri(&target.url)
                    && label_matches_url(text, &target.url)
                {
                    out.push(Piece::text(target.url.to_string()));
                } else {
                    self.extend_pieces(inlines, out);
                }
            }
            Inline::Strikeout(inlines) => {
                out.push(Piece::text("~~"));
                self.extend_pieces(inlines, out);
                out.push(Piece::text("~~"));
            }
            Inline::Superscript(inlines) => {
                let inner = pieces_to_string(&self.pieces(inlines));
                out.push(Piece::text(to_superscript(&inner)));
            }
            Inline::Subscript(inlines) => {
                let inner = pieces_to_string(&self.pieces(inlines));
                out.push(Piece::text(to_subscript(
                    &inner,
                    forces_superscript(inlines),
                )));
            }
            Inline::SmallCaps(inlines) => {
                let start = out.len();
                self.extend_pieces(inlines, out);
                uppercase_pieces(out, start);
            }
            Inline::Quoted(kind, inlines) => {
                let (open, close) = if self.smart {
                    match kind {
                        QuoteType::SingleQuote => ('\'', '\''),
                        QuoteType::DoubleQuote => ('"', '"'),
                    }
                } else {
                    quote_marks(kind)
                };
                out.push(Piece::text(open.to_string()));
                self.extend_pieces(inlines, out);
                out.push(Piece::text(close.to_string()));
            }
            Inline::Space => out.push(Piece::Space),
            Inline::SoftBreak => out.push(Piece::Soft),
            Inline::LineBreak => out.push(Piece::Hard),
            Inline::Math(kind, tex) => self.math(kind, tex, out),
            Inline::RawInline(format, text) => {
                if is_plain_format(format) {
                    out.push(Piece::text(text.to_string()));
                }
            }
            Inline::Image(_, inlines, target) => {
                out.push(Piece::text("["));
                // Alternate text that merely repeats the source URL conveys nothing, so it is dropped.
                if carta_ast::to_plain_text(inlines) != target.url {
                    self.extend_pieces(inlines, out);
                }
                out.push(Piece::text("]"));
            }
            Inline::Note(blocks) => {
                let marker = self.record_note(blocks);
                out.push(Piece::text(marker));
            }
        }
    }

    /// Render math. A convertible expression lowers to the writer-agnostic inline tree (italic
    /// variables, unicode sub/superscripts, symbols and Greek letters), which the inline renderer
    /// above turns into plain text. An expression with no single-line form is emitted verbatim,
    /// wrapped in the math delimiters of its kind (`$…$` for inline, `$$…$$` for display). Inline
    /// source has its edge whitespace trimmed before wrapping (interior whitespace is kept); display
    /// source is wrapped as written.
    ///
    /// Display math sits on its own line: a forced break frames it, absorbing any adjacent space and
    /// collapsing with a neighbouring display formula's break, so consecutive formulas each land on a
    /// separate line while a lone formula gains no surrounding blank.
    fn math(&mut self, kind: &MathType, tex: &str, out: &mut Vec<Piece>) {
        let display = matches!(kind, MathType::DisplayMath);
        if display {
            out.push(Piece::Hard);
        }
        if let Some(inlines) = crate::math::to_inlines(tex) {
            for inline in &inlines {
                self.inline(inline, out);
            }
        } else {
            let (delimiter, body) = match kind {
                MathType::InlineMath => ("$", tex.trim()),
                MathType::DisplayMath => ("$$", tex),
            };
            out.push(Piece::text(format!("{delimiter}{body}{delimiter}")));
        }
        if display {
            out.push(Piece::Hard);
        }
    }
}

impl NotesHost for State {
    fn notes(&mut self) -> &mut Vec<String> {
        &mut self.footnotes
    }

    fn render_block(&mut self, block: &Block, width: usize) -> String {
        self.block(block, width, false)
    }

    fn render_offset_paragraph(
        &mut self,
        inlines: &[Inline],
        width: usize,
        initial: usize,
    ) -> String {
        let pieces = self.pieces(inlines);
        fill_offset(&pieces, width, initial, self.wrap)
    }

    fn base_width(&self) -> usize {
        self.width
    }
}

/// Whether a raw node targets this writer and should pass its content through verbatim. Raw content
/// whose format matches `plain` (case-insensitively) is emitted; everything else is dropped.
fn is_plain_format(format: &Format) -> bool {
    format.0.eq_ignore_ascii_case("plain")
}

/// Collapse Unicode smart punctuation in a text run to its ASCII form for a `smart`-enabled render;
/// every other character passes through unchanged.
fn unsmarten(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ascii_punctuation(ch) {
            Some(ascii) => out.push_str(ascii),
            None => out.push(ch),
        }
    }
    out
}

/// Whether a single definition lays out with blank lines. A definition is rendered
/// compactly only when its first block is a [`Block::Plain`]; an empty definition or one that opens
/// with block-level content (a paragraph, list, quote, code block, …) gets blank-line spacing.
fn is_loose_definition(blocks: &[Block]) -> bool {
    !matches!(blocks.first(), Some(Block::Plain(_)))
}

/// Uppercase the text of every piece from `start` onward, in place (small-caps rendering).
fn uppercase_pieces(pieces: &mut [Piece], start: usize) {
    for piece in pieces.iter_mut().skip(start) {
        if let Piece::Text(text) = piece {
            *text = text.to_uppercase().into();
        }
    }
}

fn join_pieces(pieces: &[Piece], hard: char) -> String {
    let mut out = String::new();
    for piece in pieces {
        match piece {
            Piece::Text(text) => out.push_str(text),
            Piece::Space | Piece::Soft => out.push(' '),
            Piece::Hard => out.push(hard),
        }
    }
    out
}

fn pieces_to_string(pieces: &[Piece]) -> String {
    join_pieces(pieces, '\n')
}

/// Flatten a figure caption's blocks into one inline sequence, joining successive paragraphs with a
/// line break. An empty caption yields an empty sequence; a caption holding anything other than
/// paragraphs yields `None`.
fn figure_caption_inlines(blocks: &[Block]) -> Option<Vec<Inline>> {
    let mut inlines = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        let (Block::Plain(paragraph) | Block::Para(paragraph)) = block else {
            return None;
        };
        if index > 0 {
            inlines.push(Inline::LineBreak);
        }
        inlines.extend(paragraph.iter().cloned());
    }
    Some(inlines)
}

/// Flatten a header's inline pieces to a single line: a forced break renders as a space, keeping a
/// header on one line.
fn header_text(pieces: &[Piece]) -> String {
    join_pieces(pieces, ' ')
}

/// Render a table's attributes as a trailing `{#id .class key="value"}` suffix, or `None` when the
/// table carries no attributes.
fn attribute_suffix(attr: &Attr) -> Option<String> {
    if attr.id.is_empty() && attr.classes.is_empty() && attr.attributes.is_empty() {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    if !attr.id.is_empty() {
        parts.push(format!("#{}", attr.id));
    }
    for class in &attr.classes {
        parts.push(format!(".{class}"));
    }
    for (key, value) in &attr.attributes {
        parts.push(format!("{key}=\"{value}\""));
    }
    Some(format!("{{{}}}", parts.join(" ")))
}

#[derive(Debug, Clone, Copy)]
enum Script {
    Super,
    Sub,
}

/// Render text as superscript: when every character has a Unicode superscript equivalent (digits and
/// a small set of symbols, with spaces preserved) emit the mapped characters; otherwise fall back to
/// the parenthesized form (`^(…)`).
fn to_superscript(text: &str) -> String {
    map_script(text, Script::Super).unwrap_or_else(|| format!("^({text})"))
}

/// Render subscript text. The content is mapped to subscript glyphs when every character has one;
/// when it does not (because a character is structurally non-textual, signalled by
/// `force_superscript`, or simply lacks a subscript glyph) the whole run is mapped to *superscript*
/// glyphs instead, if that succeeds. Only a run that maps under neither script falls back to the
/// parenthesized form (`_(…)`).
fn to_subscript(text: &str, force_superscript: bool) -> String {
    if !force_superscript && let Some(mapped) = map_script(text, Script::Sub) {
        return mapped;
    }
    map_script(text, Script::Super).unwrap_or_else(|| format!("_({text})"))
}

/// Map an entire run to a single script, or `None` if any character lacks a glyph in that script.
fn map_script(text: &str, kind: Script) -> Option<String> {
    text.chars().map(|ch| script_char(ch, kind)).collect()
}

/// Whether a script's content holds an inline that is neither plain text nor a space. Such content
/// has no subscript form, so it triggers the subscript-to-superscript fallback in [`to_subscript`].
fn forces_superscript(inlines: &[Inline]) -> bool {
    inlines
        .iter()
        .any(|inline| !matches!(inline, Inline::Str(_) | Inline::Space))
}

/// Whether a character passes through the script mappers unchanged. Every space — the ASCII
/// whitespace controls (`\t \n \v \f \r`), `' '`, and any Unicode space separator (category `Zs`,
/// such as the fixed-width math spaces) — keeps a run convertible and renders as itself, while a
/// line/paragraph separator, a zero-width mark, or any other format character does not.
fn is_script_space(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\n' | '\u{000b}' | '\u{000c}' | '\r')
        || matches!(
            ch,
            '\u{00a0}' | '\u{1680}' | '\u{2000}'
                ..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}'
        )
}

fn script_char(ch: char, kind: Script) -> Option<char> {
    if is_script_space(ch) {
        return Some(ch);
    }
    let mapped = match kind {
        Script::Super => match ch {
            '0' => '\u{2070}',
            '1' => '\u{00b9}',
            '2' => '\u{00b2}',
            '3' => '\u{00b3}',
            '4' => '\u{2074}',
            '5' => '\u{2075}',
            '6' => '\u{2076}',
            '7' => '\u{2077}',
            '8' => '\u{2078}',
            '9' => '\u{2079}',
            '+' => '\u{207a}',
            '-' | '\u{2212}' => '\u{207b}',
            '=' => '\u{207c}',
            '(' => '\u{207d}',
            ')' => '\u{207e}',
            _ => return None,
        },
        Script::Sub => match ch {
            '0' => '\u{2080}',
            '1' => '\u{2081}',
            '2' => '\u{2082}',
            '3' => '\u{2083}',
            '4' => '\u{2084}',
            '5' => '\u{2085}',
            '6' => '\u{2086}',
            '7' => '\u{2087}',
            '8' => '\u{2088}',
            '9' => '\u{2089}',
            '+' => '\u{208a}',
            '-' => '\u{208b}',
            '=' => '\u{208c}',
            '(' => '\u{208d}',
            ')' => '\u{208e}',
            _ => return None,
        },
    };
    Some(mapped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use carta_ast::Document;

    fn render(blocks: Vec<Block>) -> String {
        let document = Document {
            blocks,
            ..Document::default()
        };
        PlainWriter
            .write(&document, &WriterOptions::default())
            .unwrap()
    }

    fn render_columns(blocks: Vec<Block>, columns: usize) -> String {
        let document = Document {
            blocks,
            ..Document::default()
        };
        let mut options = WriterOptions::default();
        options.columns = Some(columns);
        PlainWriter.write(&document, &options).unwrap()
    }

    #[test]
    fn deeply_nested_tables_render_without_compounding_measurement() {
        // Sizing a grid column renders each cell beyond the final emit, so without the nesting cap
        // every level would multiply the renders of all levels below it — exponential in depth.
        use carta_ast::{Alignment, Cell, ColSpec, ColWidth, Row, Table, TableBody};

        fn nested_table(content: Vec<Block>) -> Block {
            let cell = Cell {
                attr: Attr::default(),
                align: Alignment::AlignDefault,
                row_span: 1,
                col_span: 1,
                content,
            };
            let filler = Cell {
                content: vec![Block::Para(vec![Inline::Str("cell".into())])],
                ..cell.clone()
            };
            let spec = ColSpec {
                align: Alignment::AlignDefault,
                width: ColWidth::ColWidthDefault,
            };
            Block::Table(Box::new(Table {
                col_specs: vec![spec.clone(), spec],
                bodies: vec![TableBody {
                    body: vec![Row {
                        attr: Attr::default(),
                        cells: vec![cell, filler],
                    }],
                    ..TableBody::default()
                }],
                ..Table::default()
            }))
        }

        // Deep enough that compounding measurement would take minutes, while capped measurement
        // stays well under a second.
        let mut block = Block::Para(vec![Inline::Str("innermost".into())]);
        for _ in 0..9 {
            block = nested_table(vec![block]);
        }
        render(vec![block]);
    }

    fn long_paragraph() -> Vec<Block> {
        let words: Vec<Inline> = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda"
            .split(' ')
            .flat_map(|word| [Inline::Str(word.to_owned().into()), Inline::Space])
            .collect();
        vec![Block::Para(words)]
    }

    #[test]
    fn narrow_columns_wraps_a_paragraph_sooner() {
        let wide = render_columns(long_paragraph(), 40);
        let narrow = render_columns(long_paragraph(), 20);
        // A narrower fill column forces more line breaks.
        assert!(narrow.lines().count() > wide.lines().count());
        // Every laid-out line stays within the requested width.
        assert!(narrow.lines().all(|line| line.chars().count() <= 20));
        assert!(wide.lines().all(|line| line.chars().count() <= 40));
    }

    #[test]
    fn omitted_columns_uses_the_default_fill_width() {
        // The default-width render is identical to passing the built-in width explicitly.
        assert_eq!(
            render(long_paragraph()),
            render_columns(long_paragraph(), 72)
        );
    }

    fn math_para(kind: MathType, tex: &str) -> Block {
        Block::Para(vec![Inline::Math(kind, tex.to_owned().into())])
    }

    fn inline(tex: &str) -> String {
        render(vec![math_para(MathType::InlineMath, tex)])
    }

    fn display(tex: &str) -> String {
        render(vec![math_para(MathType::DisplayMath, tex)])
    }

    #[test]
    fn variable_with_superscript_uses_unicode_exponent() {
        assert_eq!(inline("a^2"), "a\u{b2}");
    }

    #[test]
    fn polynomial_lays_out_with_operator_and_relation_spacing() {
        // Binary operators take a four-per-em space (U+2005), the relation a three-per-em space
        // (U+2004), and digit exponents map to unicode superscripts.
        assert_eq!(
            inline("a^2 + b^2 = c^2"),
            "a\u{b2}\u{2005}+\u{2005}b\u{b2}\u{2004}=\u{2004}c\u{b2}"
        );
    }

    #[test]
    fn subscript_falls_back_to_parenthesized_form_for_letters() {
        // A letter index has no unicode subscript glyph, so it renders parenthesized.
        assert_eq!(inline("a_n"), "a_(n)");
    }

    #[test]
    fn greek_letters_render_as_their_codepoints() {
        assert_eq!(
            inline("\\alpha + \\beta"),
            "\u{3b1}\u{2005}+\u{2005}\u{3b2}"
        );
    }

    #[test]
    fn blackboard_bold_renders_as_letterlike_symbol() {
        assert_eq!(inline("\\mathbb{R}"), "\u{211d}");
    }

    #[test]
    fn accent_renders_as_combining_mark() {
        assert_eq!(inline("\\bar{x}"), "x\u{304}");
    }

    #[test]
    fn integral_uses_unicode_scripts_and_thin_space() {
        // The integral sign carries its limits as unicode sub/superscripts and the thin space
        // (`\,`) renders as U+2006.
        assert_eq!(
            display("\\int_0^1 x \\, dx"),
            "\u{222b}\u{2080}\u{b9}x\u{2006}dx"
        );
    }

    #[test]
    fn inline_fallback_emits_verbatim_single_dollars() {
        // A construct with no single-line form is wrapped verbatim in inline math delimiters,
        // with no escaping of the dollar signs.
        assert_eq!(inline("\\frac{1}{2}"), "$\\frac{1}{2}$");
    }

    #[test]
    fn display_fallback_emits_verbatim_double_dollars() {
        assert_eq!(display("\\sqrt{x}"), "$$\\sqrt{x}$$");
    }

    #[test]
    fn inline_fallback_trims_edge_whitespace() {
        // The verbatim inline fallback strips leading and trailing whitespace before wrapping in
        // `$…$`; interior whitespace is preserved.
        assert_eq!(inline("\\sqrt{x} "), "$\\sqrt{x}$");
        assert_eq!(inline(" \\sqrt{x}"), "$\\sqrt{x}$");
        assert_eq!(inline("  \\sqrt{x}  "), "$\\sqrt{x}$");
        assert_eq!(inline("\\sqrt{x}   y"), "$\\sqrt{x}   y$");
    }

    #[test]
    fn display_fallback_keeps_edge_whitespace() {
        // Display fallback wraps the source as written; only inline math trims its edges.
        assert_eq!(display("\\sqrt{x} "), "$$\\sqrt{x} $$");
        assert_eq!(display(" \\sqrt{x}"), "$$ \\sqrt{x}$$");
    }

    #[test]
    fn inline_fallback_of_lone_backslash() {
        // A fallback body of a lone backslash wraps to `$\$` with no escaping. A `\ ` whose
        // conversion bails to verbatim trims to this same body, so the trim composes with that bail.
        assert_eq!(inline("\\"), "$\\$");
    }

    #[test]
    fn math_flows_inside_surrounding_text() {
        let blocks = vec![Block::Para(vec![
            Inline::Str("value".to_owned().into()),
            Inline::Space,
            Inline::Math(MathType::InlineMath, "E = mc^2".to_owned().into()),
        ])];
        assert_eq!(render(blocks), "value E\u{2004}=\u{2004}mc\u{b2}");
    }

    /// Render a single subscript/superscript run from a plain string of inner text.
    fn sub(text: &str) -> String {
        render(vec![Block::Para(vec![Inline::Subscript(vec![
            Inline::Str(text.to_owned().into()),
        ])])])
    }
    fn sup(text: &str) -> String {
        render(vec![Block::Para(vec![Inline::Superscript(vec![
            Inline::Str(text.to_owned().into()),
        ])])])
    }

    #[test]
    fn ordinary_subscript_digits_use_subscript_glyphs() {
        // A run that maps under the subscript script stays subscript and is never flipped.
        assert_eq!(sub("12"), "\u{2081}\u{2082}");
        assert_eq!(sub("-1"), "\u{208b}\u{2081}"); // ASCII hyphen-minus has a subscript glyph
        assert_eq!(sub("+1"), "\u{208a}\u{2081}");
        assert_eq!(sub("=1"), "\u{208c}\u{2081}"); // U+208C subscript equals
        assert_eq!(sub("(1)"), "\u{208d}\u{2081}\u{208e}");
    }

    #[test]
    fn math_minus_has_no_subscript_glyph_so_the_run_flips_to_superscript() {
        // U+2212 is absent from the subscript script but present in the superscript script, so a
        // subscript run containing it maps wholly to superscript glyphs.
        assert_eq!(sub("\u{2212}1"), "\u{207b}\u{00b9}");
        assert_eq!(sub("\u{2212}2"), "\u{207b}\u{00b2}");
        assert_eq!(sub("\u{2212}"), "\u{207b}");
        assert_eq!(sub("1\u{2212}2"), "\u{00b9}\u{207b}\u{00b2}");
        // The superscript script maps U+2212 directly.
        assert_eq!(sup("\u{2212}1"), "\u{207b}\u{00b9}");
    }

    #[test]
    fn run_that_maps_under_neither_script_falls_back_to_parentheses() {
        // A letter beside the math minus maps under neither script, so the whole run is parenthesized.
        assert_eq!(sub("\u{2212}a"), "_(\u{2212}a)");
        assert_eq!(sub("1\u{2212}a"), "_(1\u{2212}a)");
    }

    #[test]
    fn math_spaces_pass_through_the_script_mappers_unchanged() {
        // The fixed-width math spaces keep a run convertible and render as themselves, with the
        // mappable characters around them subscripted.
        assert_eq!(
            sub("\u{2004}=\u{2004}1"),
            "\u{2004}\u{208c}\u{2004}\u{2081}"
        );
        assert_eq!(sub("1\u{2005}2"), "\u{2081}\u{2005}\u{2082}");
        assert_eq!(sub("1\u{2006}2"), "\u{2081}\u{2006}\u{2082}");
        assert_eq!(sub("1\u{2009}2"), "\u{2081}\u{2009}\u{2082}");
        assert_eq!(sub("1\u{00a0}2"), "\u{2081}\u{00a0}\u{2082}");
        assert_eq!(
            sup("\u{2004}=\u{2004}1"),
            "\u{2004}\u{207c}\u{2004}\u{00b9}"
        );
    }

    #[test]
    fn non_space_separators_and_format_marks_do_not_pass_through() {
        // A line/paragraph separator or zero-width mark is not a space, so it forces the fallback.
        assert!(!is_script_space('\u{2028}')); // line separator (Zl)
        assert!(!is_script_space('\u{2029}')); // paragraph separator (Zp)
        assert!(!is_script_space('\u{200b}')); // zero-width space (Cf)
        assert!(!is_script_space('\u{0085}')); // next line (Cc)
        assert!(!is_script_space('\u{feff}')); // byte-order mark (Cf)
        assert_eq!(sub("1\u{2028}2"), "_(1\u{2028}2)");
        // Every ASCII whitespace control and Unicode space separator does pass through.
        for ch in [
            ' ', '\t', '\n', '\u{000b}', '\u{000c}', '\r', '\u{00a0}', '\u{1680}', '\u{2000}',
            '\u{200a}', '\u{202f}', '\u{205f}', '\u{3000}',
        ] {
            assert!(
                is_script_space(ch),
                "expected {:#x} to be a script space",
                ch as u32
            );
        }
    }

    #[test]
    fn formatted_subscript_content_flips_the_whole_run_to_superscript() {
        // Content that is not plain text or a space has no subscript form, so a convertible run is
        // rendered with superscript glyphs.
        let flipped = render(vec![Block::Para(vec![Inline::Subscript(vec![
            Inline::Emph(vec![Inline::Str("2".to_owned().into())]),
        ])])]);
        assert_eq!(flipped, "\u{00b2}");
        // A formatted but otherwise unmappable run still falls back.
        let fallback = render(vec![Block::Para(vec![Inline::Subscript(vec![
            Inline::Emph(vec![Inline::Str("a".to_owned().into())]),
        ])])]);
        assert_eq!(fallback, "_(a)");
    }

    #[test]
    fn absurd_column_width_stays_bounded() {
        use carta_ast::{Caption, Cell, ColSpec, TableBody, TableFoot, TableHead};
        let cell = Cell {
            attr: Attr::default(),
            align: Alignment::AlignLeft,
            row_span: 1,
            col_span: 1,
            content: vec![Block::Para(vec![Inline::Str("x".to_owned().into())])],
        };
        let table = Table {
            attr: Attr::default(),
            caption: Caption::default(),
            col_specs: vec![ColSpec {
                align: Alignment::AlignLeft,
                width: ColWidth::ColWidth(1.9e53),
            }],
            head: TableHead::default(),
            bodies: vec![TableBody {
                attr: Attr::default(),
                row_head_columns: 0,
                head: Vec::new(),
                body: vec![Row {
                    attr: Attr::default(),
                    cells: vec![cell],
                }],
            }],
            foot: TableFoot::default(),
        };
        // A fractional spec far past the whole line must not inflate the rule into a huge
        // allocation; the output stays within a handful of line widths.
        let output = render(vec![Block::Table(Box::new(table))]);
        assert!(
            output.len() < 1_000,
            "unbounded table output: {} bytes",
            output.len()
        );
    }
}
