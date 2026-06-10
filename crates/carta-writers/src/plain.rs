//! Plain-text writer: renders the document model to unformatted text.
//!
//! Output uses a fill column of 72, strips inline markup (emphasis, links, and inline code render as
//! their textual content), and conveys block structure through indentation alone. It carries no
//! trailing newline; the caller appends one. This format has no public specification.

use carta_ast::{
    Alignment, Attr, Block, Cell, ColWidth, Document, Format, Inline, ListAttributes, Row, Table,
};
use carta_core::{Result, Writer, WriterOptions};

use crate::common::{
    FILL_COLUMN, NotesHost, Piece, append_notes, display_width, fill, fill_offset, indent_block,
    is_loose, item_separator, join_loose, offset_as_i32, ordered_marker, quote_marks,
};
use crate::grid;

/// Renders a document to plain text.
#[derive(Debug, Default, Clone, Copy)]
pub struct PlainWriter;

impl Writer for PlainWriter {
    fn write(&self, document: &Document, _options: &WriterOptions) -> Result<String> {
        let mut state = State::default();
        let body = state.blocks_to_string(&document.blocks, FILL_COLUMN);
        Ok(append_notes(body, &state.footnotes))
    }
}

/// Carries the footnote bodies accumulated while rendering, so notes can be collected inline and
/// emitted as a section at the end of the document.
#[derive(Debug, Default)]
struct State {
    footnotes: Vec<String>,
}

impl State {
    /// Render a block sequence with a blank line between blocks, dropping those that produce no
    /// output. This is the default layout (document body, block quotes, divs, figures, loose list
    /// items, loose definitions). See [`join_loose`] for the [`Block::Plain`] spacing quirk.
    fn blocks_to_string(&mut self, blocks: &[Block], width: usize) -> String {
        let rendered = blocks
            .iter()
            .map(|block| (matches!(block, Block::Plain(_)), self.block(block, width)))
            .collect();
        join_loose(rendered)
    }

    /// Render a block sequence with a single newline between blocks: the compact layout used inside a
    /// tight list's items and tight definitions.
    fn blocks_tight(&mut self, blocks: &[Block], width: usize) -> String {
        let parts: Vec<String> = blocks
            .iter()
            .map(|block| self.block(block, width))
            .filter(|rendered| !rendered.is_empty())
            .collect();
        parts.join("\n")
    }

    /// Render a block sequence at the given layout density.
    fn blocks_at(&mut self, blocks: &[Block], width: usize, loose: bool) -> String {
        if loose {
            self.blocks_to_string(blocks, width)
        } else {
            self.blocks_tight(blocks, width)
        }
    }

    fn block(&mut self, block: &Block, width: usize) -> String {
        match block {
            Block::Plain(inlines) | Block::Para(inlines) => {
                let pieces = self.pieces(inlines);
                fill(&pieces, width)
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
                let body = self.blocks_to_string(blocks, width.saturating_sub(2));
                indent_block(&body, "  ", "  ")
            }
            Block::BulletList(items) => self.bullet_list(items, width),
            Block::OrderedList(attrs, items) => self.ordered_list(attrs, items, width),
            Block::DefinitionList(items) => self.definition_list(items, width),
            Block::HorizontalRule => "-".repeat(FILL_COLUMN),
            Block::Table(table) => self.table(table),
            Block::Figure(_, _, blocks) | Block::Div(_, blocks) => {
                self.blocks_to_string(blocks, width)
            }
            Block::LineBlock(lines) => self.line_block(lines),
        }
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
                let body = self.blocks_at(item, body_width, loose);
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
                let marker = ordered_marker(number, &attrs.style, &attrs.delim);
                let field = (marker.chars().count() + 1).max(4);
                let body = self.blocks_at(item, width.saturating_sub(field), loose);
                let first = format!("{marker:<field$}");
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
                let mut group = fill(&term_pieces, width);
                for definition in definitions {
                    let loose = is_loose_definition(definition);
                    let body = self.blocks_at(definition, width.saturating_sub(2), loose);
                    let indented = indent_block(&body, "  ", "  ");
                    group.push_str(if loose { "\n\n" } else { "\n" });
                    group.push_str(&indented);
                }
                group
            })
            .collect();
        groups.join("\n\n")
    }

    fn table(&mut self, table: &Table) -> String {
        if table.col_specs.is_empty() {
            return String::new();
        }
        let form = table_form(table);
        let body = match form {
            TableForm::Simple => self.simple_table(table),
            TableForm::Multiline => self.multiline_table(table),
            TableForm::Grid => self.grid_table(table),
        };
        match self.table_caption(table, form) {
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
    fn multiline_table(&mut self, table: &Table) -> String {
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
                let (width, word) = measure_pieces(cell);
                if let Some(value) = natural.get_mut(index) {
                    *value = (*value).max(width);
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
                        // A bounded fraction scaled to a small column width: `floor`/`max(0.0)`
                        // make the conversion exact and non-negative.
                        #[allow(
                            clippy::cast_precision_loss,
                            clippy::cast_possible_truncation,
                            clippy::cast_sign_loss
                        )]
                        let scaled = (fraction * MULTILINE_WIDTH as f64).floor().max(0.0) as usize;
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
    fn grid_table(&mut self, table: &Table) -> String {
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

        let snapshot = self.footnotes.len();
        let mut natural = vec![0usize; columns];
        let mut minword = vec![0usize; columns];
        for (rows, layout) in [
            (&head, &head_layout),
            (&body, &body_layout),
            (&foot, &foot_layout),
        ] {
            self.measure_grid(rows, layout, &mut natural, &mut minword);
        }
        self.footnotes.truncate(snapshot);

        let colspans: Vec<(usize, usize)> = [&head_layout, &body_layout, &foot_layout]
            .into_iter()
            .flatten()
            .flatten()
            .copied()
            .filter(|&(_, span)| span > 1)
            .collect();
        let content =
            grid::grid_content_widths(&table.col_specs, &natural, &minword, &colspans, columns);
        let col_widths: Vec<usize> = content.iter().map(|width| width + 2).collect();
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
                *slot = join_pieces(&pieces, ' ');
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
        let text = self.blocks_to_string(content, width);
        if text.is_empty() {
            Vec::new()
        } else {
            text.split('\n').map(str::to_owned).collect()
        }
    }

    /// The caption block, prefixed `: ` and indented to match the table form (two columns for
    /// simple and multiline tables, none for grids). A non-empty caption carries any table
    /// attributes as a trailing `{#id .class key="value"}` suffix.
    fn table_caption(&mut self, table: &Table, form: TableForm) -> Option<String> {
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
            pieces.push(Piece::Text(suffix));
        }
        let body = fill_offset(&pieces, FILL_COLUMN.saturating_sub(base), 2);
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
                out.push(Piece::Text(format!("{prefix}\\!")));
                continue;
            }
            self.inline(inline, out);
        }
    }

    fn inline(&mut self, inline: &Inline, out: &mut Vec<Piece>) {
        match inline {
            Inline::Str(text) | Inline::Code(_, text) => out.push(Piece::Text(text.clone())),
            Inline::Emph(inlines)
            | Inline::Strong(inlines)
            | Inline::Underline(inlines)
            | Inline::Cite(_, inlines)
            | Inline::Link(_, inlines, _)
            | Inline::Span(_, inlines) => self.extend_pieces(inlines, out),
            Inline::Strikeout(inlines) => {
                out.push(Piece::Text("~~".to_owned()));
                self.extend_pieces(inlines, out);
                out.push(Piece::Text("~~".to_owned()));
            }
            Inline::Superscript(inlines) => {
                let inner = pieces_to_string(&self.pieces(inlines));
                out.push(Piece::Text(to_superscript(&inner)));
            }
            Inline::Subscript(inlines) => {
                let inner = pieces_to_string(&self.pieces(inlines));
                out.push(Piece::Text(to_subscript(
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
                let (open, close) = quote_marks(kind);
                out.push(Piece::Text(open.to_string()));
                self.extend_pieces(inlines, out);
                out.push(Piece::Text(close.to_string()));
            }
            Inline::Space | Inline::SoftBreak => out.push(Piece::Space),
            Inline::LineBreak => out.push(Piece::Hard),
            Inline::Math(_, _) => todo!("plain writer: render math"),
            Inline::RawInline(format, text) => {
                if is_plain_format(format) {
                    out.push(Piece::Text(text.clone()));
                }
            }
            Inline::Image(_, inlines, _) => {
                out.push(Piece::Text("[".to_owned()));
                self.extend_pieces(inlines, out);
                out.push(Piece::Text("]".to_owned()));
            }
            Inline::Note(blocks) => {
                let marker = self.record_note(blocks);
                out.push(Piece::Text(marker));
            }
        }
    }
}

impl NotesHost for State {
    fn notes(&mut self) -> &mut Vec<String> {
        &mut self.footnotes
    }

    fn render_block(&mut self, block: &Block, width: usize) -> String {
        self.block(block, width)
    }

    fn render_offset_paragraph(
        &mut self,
        inlines: &[Inline],
        width: usize,
        initial: usize,
    ) -> String {
        let pieces = self.pieces(inlines);
        fill_offset(&pieces, width, initial)
    }
}

/// Whether a raw node targets this writer and should pass its content through verbatim. Raw content
/// whose format matches `plain` (case-insensitively) is emitted; everything else is dropped.
fn is_plain_format(format: &Format) -> bool {
    format.0.eq_ignore_ascii_case("plain")
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
            *text = text.to_uppercase();
        }
    }
}

/// Flatten inline pieces to a single string without line filling: breakable spaces become one
/// space, while forced breaks become `hard`. Used where content is not wrapped
/// (line-block lines and the inner text of sub/superscripts use a newline; see [`header_text`]).
fn join_pieces(pieces: &[Piece], hard: char) -> String {
    let mut out = String::new();
    for piece in pieces {
        match piece {
            Piece::Text(text) => out.push_str(text),
            Piece::Space => out.push(' '),
            Piece::Hard => out.push(hard),
        }
    }
    out
}

fn pieces_to_string(pieces: &[Piece]) -> String {
    join_pieces(pieces, '\n')
}

/// Flatten a header's inline pieces to a single line: a forced break renders as a space, keeping a
/// header on one line.
fn header_text(pieces: &[Piece]) -> String {
    join_pieces(pieces, ' ')
}

/// Width used to render a grid cell when measuring its natural extent, before column widths are
/// fixed: large enough that no reflow occurs.
const MEASURE_WIDTH: usize = 100_000;

/// The character budget a fractional column width scales against in a multiline table.
const MULTILINE_WIDTH: usize = 71;

#[derive(Clone, Copy)]
enum TableForm {
    Simple,
    Multiline,
    Grid,
}

/// Choose the rendering form for a table. Spans, block-level cell content, or a footer demand a
/// grid; an explicit column width or a forced break within a cell demands the multiline form;
/// otherwise the compact simple form suffices.
fn table_form(table: &Table) -> TableForm {
    let rows: Vec<&Row> = table
        .head
        .rows
        .iter()
        .chain(
            table
                .bodies
                .iter()
                .flat_map(|body| body.head.iter().chain(body.body.iter())),
        )
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
fn is_simple_cell(cell: &Cell) -> bool {
    matches!(
        cell.content.as_slice(),
        [] | [Block::Plain(_) | Block::Para(_)]
    )
}

/// The inline content of a simple cell, or an empty slice for anything richer.
fn cell_inlines(cell: &Cell) -> &[Inline] {
    match cell.content.first() {
        Some(Block::Plain(inlines) | Block::Para(inlines)) => inlines,
        _ => &[],
    }
}

/// Whether a simple cell contains a forced line break, which forces the multiline form.
fn cell_has_break(cell: &Cell) -> bool {
    is_simple_cell(cell)
        && cell_inlines(cell)
            .iter()
            .any(|inline| matches!(inline, Inline::LineBreak))
}

/// The inline content of a block, or an empty slice for a block that carries none directly.
fn block_inlines(block: &Block) -> &[Inline] {
    match block {
        Block::Plain(inlines) | Block::Para(inlines) => inlines,
        _ => &[],
    }
}

/// Every row of every body, intermediate head rows included, in document order.
fn body_rows(table: &Table) -> Vec<&Row> {
    table
        .bodies
        .iter()
        .flat_map(|body| body.head.iter().chain(body.body.iter()))
        .collect()
}

/// A row of column underlines: a run of dashes per column width, joined by single spaces.
fn dash_rule(field: &[usize]) -> String {
    field
        .iter()
        .map(|width| "-".repeat(*width))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Pad `text` to `width`, placing the slack according to the column's alignment.
fn pad_align(text: &str, width: usize, align: &Alignment) -> String {
    let used = display_width(text);
    let pad = width.saturating_sub(used);
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
fn lay_row(cells: &[Vec<String>], field: &[usize], aligns: &[&Alignment]) -> Vec<String> {
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
fn filled_cells(row: &[Vec<Piece>], field: &[usize]) -> Vec<Vec<String>> {
    row.iter()
        .enumerate()
        .map(|(index, pieces)| {
            let width = field.get(index).copied().unwrap_or(0);
            let text = fill(pieces, width);
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
fn extend_multiline_body(
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
fn indent_lines(lines: &[String], indent: usize) -> String {
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
fn measure_pieces(pieces: &[Piece]) -> (usize, usize) {
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
            Piece::Space => {
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
fn pieces_nonempty(pieces: &[Piece]) -> bool {
    pieces
        .iter()
        .any(|piece| matches!(piece, Piece::Text(text) if !text.is_empty()))
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
    let mapped: Option<String> = text
        .chars()
        .map(|ch| script_char(ch, Script::Super))
        .collect();
    mapped.unwrap_or_else(|| format!("^({text})"))
}

/// Render subscript text: when the content carries any
/// formatted inline (anything other than plain text or spaces) and is otherwise convertible, the
/// characters are mapped to their *superscript* equivalents rather than subscript ones. A
/// non-convertible run still falls back to the subscript parenthesized form.
fn to_subscript(text: &str, force_superscript: bool) -> String {
    let kind = if force_superscript {
        Script::Super
    } else {
        Script::Sub
    };
    let mapped: Option<String> = text.chars().map(|ch| script_char(ch, kind)).collect();
    mapped.unwrap_or_else(|| format!("_({text})"))
}

/// Whether a script's content holds an inline that is neither plain text nor a space. Such content
/// triggers the subscript-to-superscript fallback in [`to_subscript`].
fn forces_superscript(inlines: &[Inline]) -> bool {
    inlines
        .iter()
        .any(|inline| !matches!(inline, Inline::Str(_) | Inline::Space))
}

fn script_char(ch: char, kind: Script) -> Option<char> {
    if ch == ' ' {
        return Some(' ');
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
            '-' => '\u{207b}',
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
