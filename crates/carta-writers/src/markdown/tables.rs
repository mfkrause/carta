//! Table rendering for the markdown engine: pipe, simple, multiline, grid, and HTML forms.

use carta_ast::{Alignment, Attr, Block, Caption, Cell, ColWidth, Inline, Row, Table};
use carta_core::Extension;

use crate::common::{
    MEASURE_WIDTH, Piece, TableForm, block_inlines, body_rows, boundary_space_count, cell_inlines,
    dash_rule, display_width, extend_multiline_body, fill, fill_offset, filled_cells, indent_block,
    indent_lines, is_simple_cell, lay_row, measure_pieces, pad_align, pieces_nonempty, table_form,
    trimmed_cell_inlines,
};
use crate::grid;
use crate::markdown_common::attr_is_empty;

use super::{State, attr_braces};

/// A simple-form table cell rendered to one line: the text with boundary spaces trimmed away, and
/// the width the cell's column must reserve: the rendered width plus the trimmed spaces.
#[derive(Debug, Default)]
struct SimpleCell {
    text: String,
    sizing_width: usize,
}

impl State {
    pub(super) fn table(&mut self, table: &Table, width: usize) -> String {
        let native = self.config.has(Extension::SimpleTables)
            || self.config.has(Extension::MultilineTables)
            || self.config.has(Extension::GridTables);
        if !native {
            if self.config.has(Extension::PipeTables) {
                return self.github_table(table, width);
            }
            return self.html_table(table);
        }
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

    /// Render a table as a raw HTML block, for a dialect whose extension set gives it no native
    /// table syntax. A raw HTML block in markdown is terminated by a blank line, so any blank line
    /// the table's HTML spans (an empty body, the gap between two row groups) is collapsed by
    /// [`encode_html_block_blank_lines`] so the whole table stays a single block.
    fn html_table(&self, table: &Table) -> String {
        let html =
            crate::html::render_fragment(&[Block::Table(Box::new(table.clone()))], self.wrap);
        encode_html_block_blank_lines(&html)
    }

    /// A GitHub table: a pipe table when every cell is a single line and no cell spans, otherwise an
    /// HTML table. A column-aligned pipe table whose columns together exceed the fill column drops to
    /// a narrow form with single-space cell padding. The caption follows the table as its own block.
    fn github_table(&mut self, table: &Table, width: usize) -> String {
        if !pipe_representable(table) {
            return self.html_table(table);
        }
        let columns = table.col_specs.len();
        if columns == 0 {
            return "||\n||".to_owned();
        }
        let aligns: Vec<Alignment> = table
            .col_specs
            .iter()
            .map(|spec| spec.align.clone())
            .collect();
        let head_rows: Vec<&Row> = table.head.rows.iter().collect();
        let data_rows: Vec<&Row> = body_rows(table)
            .into_iter()
            .chain(table.foot.rows.iter())
            .collect();
        let header = head_rows.first();
        let header_cells = self.pipe_cells(header.map(|row| row.cells.as_slice()), columns);
        let data: Vec<Vec<String>> = data_rows
            .iter()
            .map(|row| self.pipe_cells(Some(&row.cells), columns))
            .collect();
        let mut col_widths = vec![3usize; columns];
        for cells in std::iter::once(&header_cells).chain(data.iter()) {
            for (index, cell) in cells.iter().enumerate() {
                if let Some(slot) = col_widths.get_mut(index) {
                    *slot = (*slot).max(display_width(cell));
                }
            }
        }
        let narrow = col_widths.iter().sum::<usize>() > width;
        let (row_widths, sep_widths) = if narrow {
            (vec![0usize; columns], vec![2usize; columns])
        } else {
            (col_widths.clone(), col_widths)
        };
        let mut lines = vec![pipe_row(&header_cells, &row_widths, &aligns)];
        lines.push(pipe_separator(&sep_widths, &aligns));
        for cells in &data {
            lines.push(pipe_row(cells, &row_widths, &aligns));
        }
        let table_text = lines.join("\n");
        match self.github_caption(&table.caption, &table.attr, width) {
            Some(caption) => format!("{table_text}\n\n{caption}"),
            None => table_text,
        }
    }

    /// The GitHub-table caption: the caption blocks reflowed and concatenated with a hard break
    /// between blocks, carrying any table attributes as a trailing `{#id .class key="value"}`
    /// suffix. `None` when the caption is empty.
    fn github_caption(&mut self, caption: &Caption, attr: &Attr, width: usize) -> Option<String> {
        let mut pieces: Vec<Piece> = Vec::new();
        for block in &caption.long {
            if !pieces.is_empty() {
                pieces.push(Piece::text(self.config.hard_break()));
                pieces.push(Piece::Hard);
            }
            self.extend_pieces(block_inlines(block), &mut pieces);
        }
        if !pieces_nonempty(&pieces) {
            return None;
        }
        if let Some(suffix) = attribute_suffix(attr) {
            pieces.push(Piece::Space);
            pieces.push(Piece::text(suffix));
        }
        Some(fill(&pieces, width, self.wrap))
    }

    /// Render the cells of one pipe-table row to single-line strings, padding the row out to the
    /// column count with empty cells.
    fn pipe_cells(&mut self, cells: Option<&[Cell]>, columns: usize) -> Vec<String> {
        let mut out = Vec::with_capacity(columns);
        let cells = cells.unwrap_or(&[]);
        for index in 0..columns {
            let text = cells
                .get(index)
                .map(|cell| self.cell_oneline(cell))
                .unwrap_or_default();
            out.push(text);
        }
        out
    }

    /// Render a cell's content to a single line for a pipe table, escaping the cell delimiter.
    /// Boundary spaces stay in the text: a pipe cell renders its content verbatim, and the column
    /// is sized to the full text.
    fn cell_oneline(&mut self, cell: &Cell) -> String {
        let inlines = cell_inlines(cell);
        self.inlines_oneline(inlines).replace('|', "\\|")
    }

    /// A simple table: one line per cell, the column width sized to the widest cell plus two. A
    /// non-empty header is underlined with a per-column dash rule; a headerless table is fenced by
    /// dash rules above and below. Indented two columns.
    fn simple_table(&mut self, table: &Table) -> String {
        let columns = table.col_specs.len();
        let aligns: Vec<&Alignment> = table.col_specs.iter().map(|spec| &spec.align).collect();
        let header: Vec<Vec<SimpleCell>> = table
            .head
            .rows
            .iter()
            .map(|row| self.simple_row(row, columns))
            .collect();
        let body: Vec<Vec<SimpleCell>> = body_rows(table)
            .iter()
            .map(|row| self.simple_row(row, columns))
            .collect();
        let has_header = header
            .iter()
            .any(|row| row.iter().any(|cell| !cell.text.is_empty()));

        let mut field = vec![0usize; columns];
        for row in header.iter().chain(body.iter()) {
            for (index, cell) in row.iter().enumerate() {
                if let Some(width) = field.get_mut(index) {
                    *width = (*width).max(cell.sizing_width + 2);
                }
            }
        }
        let rule = dash_rule(&field);
        let mut lines: Vec<String> = Vec::new();
        let lay = |row: &[SimpleCell]| {
            let cells: Vec<Vec<String>> = row.iter().map(|cell| vec![cell.text.clone()]).collect();
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

    /// Render a row's cells to single lines, one per column, padding a short row with empty cells.
    /// A cell's boundary spaces are trimmed from the rendered text but still counted toward its
    /// sizing width, so the column reserves their room.
    fn simple_row(&mut self, row: &Row, columns: usize) -> Vec<SimpleCell> {
        let mut out: Vec<SimpleCell> = (0..columns).map(|_| SimpleCell::default()).collect();
        for (index, cell) in row.cells.iter().enumerate() {
            if let Some(slot) = out.get_mut(index) {
                let text = self.inlines_oneline(trimmed_cell_inlines(cell));
                let sizing_width = display_width(&text) + boundary_space_count(cell);
                *slot = SimpleCell { text, sizing_width };
            }
        }
        out
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
                        // `floor`/`max(0.0)` keep the conversion exact and non-negative; `min(width)`
                        // clamps a fraction past the whole line, which would allocate a rule that long.
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

    /// A bordered grid table built on the shared grid engine, with content widths from explicit
    /// fractional specs when present and a content-proportional fit otherwise.
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
            // Column measurement re-renders every cell, compounding per nesting level; past the cap,
            // columns take an even share of the fill width, keeping total work linear.
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

    fn cell_lines(&mut self, content: &[Block], width: usize) -> Vec<String> {
        let rendered = self.blocks_to_string(content, width.max(1));
        if rendered.is_empty() {
            Vec::new()
        } else {
            rendered.split('\n').map(str::to_owned).collect()
        }
    }

    /// The caption block, prefixed `: ` and indented to match the table form (two columns for simple
    /// and multiline tables, none for grids). A non-empty caption carries any table attributes as a
    /// trailing `{#id .class key="value"}` suffix.
    fn table_caption(&mut self, table: &Table, form: TableForm, width: usize) -> Option<String> {
        let base = if matches!(form, TableForm::Grid) {
            0
        } else {
            2
        };
        let mut pieces: Vec<Piece> = Vec::new();
        for block in &table.caption.long {
            if !pieces.is_empty() {
                pieces.push(Piece::text(self.config.hard_break()));
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
}

/// Encode the blank lines of a raw HTML fragment so it survives as one raw HTML block in markdown.
/// A blank line ends a raw HTML block, so the newline that opens one (any newline directly
/// following another) is rewritten as the `&#10;` character reference, leaving single line breaks
/// untouched. This keeps an HTML table embedded in a markdown dialect with no native table syntax
/// intact as a single raw block.
fn encode_html_block_blank_lines(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut prev_newline = false;
    for ch in html.chars() {
        if ch == '\n' && prev_newline {
            out.push_str("&#10;");
        } else {
            out.push(ch);
        }
        prev_newline = ch == '\n';
    }
    out
}

/// Whether a table can render as a pipe table: every cell holds at most one paragraph of inline
/// content that fits on a single line (no forced break), and no cell spans more than one row or
/// column. A table failing this falls back to an HTML render.
fn pipe_representable(table: &Table) -> bool {
    let rows = table
        .head
        .rows
        .iter()
        .chain(body_rows(table))
        .chain(table.foot.rows.iter());
    rows.flat_map(|row| row.cells.iter()).all(|cell| {
        cell.row_span <= 1
            && cell.col_span <= 1
            && is_simple_cell(cell)
            && !cell_inlines(cell)
                .iter()
                .any(|inline| matches!(inline, Inline::LineBreak))
    })
}

/// Render a table's attributes as a trailing `{#id .class key="value"}` suffix, or `None` when the
/// table carries no attributes.
fn attribute_suffix(attr: &Attr) -> Option<String> {
    if attr_is_empty(attr) {
        return None;
    }
    Some(attr_braces(attr))
}

/// One pipe-table row: each cell padded to its column width and wrapped in `| … |`. Alignment
/// controls the padding side.
fn pipe_row(cells: &[String], widths: &[usize], aligns: &[Alignment]) -> String {
    let mut out = String::from("|");
    for (index, width) in widths.iter().enumerate() {
        let text = cells.get(index).map_or("", String::as_str);
        let align = aligns
            .get(index)
            .cloned()
            .unwrap_or(Alignment::AlignDefault);
        out.push(' ');
        out.push_str(&pad_align(text, *width, &align));
        out.push_str(" |");
    }
    out
}

/// The pipe-table alignment separator row: a dash run per column, with colons marking each column's
/// alignment, padded to the column width.
fn pipe_separator(widths: &[usize], aligns: &[Alignment]) -> String {
    let mut out = String::from("|");
    for (index, &width) in widths.iter().enumerate() {
        let align = aligns
            .get(index)
            .cloned()
            .unwrap_or(Alignment::AlignDefault);
        out.push_str(&pipe_dashes(width, &align));
        out.push('|');
    }
    out
}

/// One column's alignment-separator field: a dash run spanning the column's full interior width
/// (`width + 2`, matching a content field's surrounding spaces), with colons replacing the edge
/// dashes per alignment and no surrounding padding.
fn pipe_dashes(width: usize, align: &Alignment) -> String {
    let interior = width + 2;
    let mut field = String::with_capacity(interior);
    match align {
        Alignment::AlignLeft => {
            field.push(':');
            field.push_str(&"-".repeat(interior.saturating_sub(1)));
        }
        Alignment::AlignRight => {
            field.push_str(&"-".repeat(interior.saturating_sub(1)));
            field.push(':');
        }
        Alignment::AlignCenter => {
            field.push(':');
            field.push_str(&"-".repeat(interior.saturating_sub(2)));
            field.push(':');
        }
        Alignment::AlignDefault => field.push_str(&"-".repeat(interior)),
    }
    field
}
