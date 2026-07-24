//! Table rendering for the reStructuredText writer: simple and grid layouts.

use carta_ast::{Block, Caption, ColWidth, Inline, Row, Table};

use crate::common::{block_inlines, body_rows, display_width, indent_block};
use crate::grid;
use crate::grid::MAX_MEASURED_TABLE_NESTING;

use super::State;

impl State {
    /// Render a table. A non-empty caption becomes a `.. table::` directive followed by the table
    /// indented three columns; without one the table sits at the left margin. The simple form is
    /// chosen when the table is span-free, carries no explicit column widths, and fits the fill
    /// column; otherwise the bordered grid form is used.
    pub(super) fn table(&mut self, table: &Table, width: usize) -> String {
        let columns = table.col_specs.len();
        self.table_depth += 1;
        let body = if columns == 0 {
            String::new()
        } else if self.table_depth > MAX_MEASURED_TABLE_NESTING {
            self.grid_table(table, columns)
        } else {
            match self.simple_layout(table, columns) {
                Some(widths) => self.simple_table(table, &widths),
                None => self.grid_table(table, columns),
            }
        };
        self.table_depth = self.table_depth.saturating_sub(1);
        match self.table_caption(&table.caption, width) {
            Some(caption) if body.is_empty() => caption,
            Some(caption) => format!("{caption}\n\n{}", indent_block(&body, "   ", "   ")),
            None => body,
        }
    }

    /// Decide whether the table renders in simple form, returning its per-column content widths when
    /// so. A single column, a row span, or an explicit column width forces the grid form; otherwise
    /// the simple form is used only when the laid-out width fits the fill column.
    fn simple_layout(&mut self, table: &Table, columns: usize) -> Option<Vec<usize>> {
        if columns <= 1 {
            return None;
        }
        let rows: Vec<&Row> = table
            .head
            .rows
            .iter()
            .chain(body_rows(table))
            .chain(table.foot.rows.iter())
            .collect();
        if rows.is_empty() {
            return None;
        }
        let has_rowspan = rows
            .iter()
            .any(|row| row.cells.iter().any(|cell| cell.row_span > 1));
        let has_explicit = table
            .col_specs
            .iter()
            .any(|spec| matches!(spec.width, ColWidth::ColWidth(fraction) if fraction > 0.0));
        if has_rowspan || has_explicit {
            return None;
        }
        let widths = self.simple_widths(&rows, columns);
        let total = widths.iter().sum::<usize>() + columns.saturating_sub(1);
        if total > self.width {
            None
        } else {
            Some(widths)
        }
    }

    /// The natural display width of each column: the widest single-column cell, with a spanning
    /// cell's whole content absorbed into the first column it covers.
    fn simple_widths(&mut self, rows: &[&Row], columns: usize) -> Vec<usize> {
        let mut widths = vec![0usize; columns];
        let snapshot = self.snapshot();
        for row in rows {
            let mut col = 0;
            for cell in &row.cells {
                if col >= columns {
                    break;
                }
                let span = grid::span_count(cell.col_span).min(columns - col);
                let lines = self.cell_lines(&cell.content, SIMPLE_WIDTH);
                let mut content = lines
                    .iter()
                    .map(|line| display_width(line))
                    .max()
                    .unwrap_or(0);
                // A trailing cell space is trimmed from the field but still holds a column of width.
                if cell_ends_with_space(&cell.content) {
                    content += 1;
                }
                if let Some(slot) = widths.get_mut(col) {
                    *slot = (*slot).max(content);
                }
                col += span;
            }
        }
        self.restore(snapshot);
        widths
    }

    /// A simple table: `=` rules above and below, plus one under a non-empty header. Cells render at
    /// their natural width with no wrapping; a column-spanning cell occupies its merged field, and a
    /// row holding one is followed by a `-` underline. Not indented.
    fn simple_table(&mut self, table: &Table, widths: &[usize]) -> String {
        let columns = widths.len();
        let head: Vec<&Row> = table.head.rows.iter().collect();
        let data: Vec<&Row> = body_rows(table)
            .into_iter()
            .chain(table.foot.rows.iter())
            .collect();
        let has_header = head
            .iter()
            .any(|row| row.cells.iter().any(|cell| !cell.content.is_empty()));
        let rule = equals_rule(widths);

        let mut lines: Vec<String> = vec![rule.clone()];
        for row in &head {
            let after_rule = lines.last() == Some(&rule);
            self.simple_row(row, widths, columns, after_rule, &mut lines);
        }
        if has_header {
            lines.push(rule.clone());
        }
        for row in &data {
            let after_rule = lines.last() == Some(&rule);
            self.simple_row(row, widths, columns, after_rule, &mut lines);
        }
        lines.push(rule);
        lines.join("\n")
    }

    /// Append one simple-table row: each cell's lines stacked across the column fields, the last
    /// column left unpadded so trailing space is dropped, then a `-` underline when the row carries a
    /// column span.
    fn simple_row(
        &mut self,
        row: &Row,
        widths: &[usize],
        columns: usize,
        after_rule: bool,
        lines: &mut Vec<String>,
    ) {
        let mut col_lines: Vec<Vec<String>> = vec![Vec::new(); columns];
        let mut placements: Vec<(usize, usize)> = Vec::new();
        let mut col = 0;
        for cell in &row.cells {
            if col >= columns {
                break;
            }
            let span = grid::span_count(cell.col_span).min(columns - col);
            if let Some(slot) = col_lines.get_mut(col) {
                *slot = self.cell_lines(&cell.content, SIMPLE_WIDTH);
            }
            placements.push((col, span));
            col += span;
        }
        // A row right after a `=` rule with an empty first cell would start with whitespace, which
        // the grammar reads as rule continuation; a lone backslash holds the column open.
        let row_has_content = col_lines.iter().any(|lines| !lines.is_empty());
        if after_rule
            && row_has_content
            && let Some(first) = col_lines.first_mut()
            && first.is_empty()
        {
            first.push("\\".to_owned());
        }
        let height = col_lines.iter().map(Vec::len).max().unwrap_or(0).max(1);
        for line in 0..height {
            lines.push(lay_simple_line(&col_lines, widths, columns, line));
        }
        if placements.iter().any(|&(_, span)| span > 1) {
            lines.push(colspan_underline(&placements, widths));
        }
    }

    /// A grid table: bordered cells whose widths come from explicit fractional specs or a
    /// content-proportional fit; the engine in [`crate::grid`] draws the borders without alignment
    /// colons. Not indented.
    fn grid_table(&mut self, table: &Table, columns: usize) -> String {
        let head: Vec<&Row> = table.head.rows.iter().collect();
        let body = body_rows(table);
        let foot: Vec<&Row> = table.foot.rows.iter().collect();
        let head_layout = grid::place_columns(&head, columns);
        let body_layout = grid::place_columns(&body, columns);
        let foot_layout = grid::place_columns(&foot, columns);

        let mut natural = vec![0usize; columns];
        let mut minword = vec![0usize; columns];
        if self.table_depth > MAX_MEASURED_TABLE_NESTING {
            // Measuring re-renders every cell, compounding per nesting level; past the cap,
            // columns take an even share of the fill width, keeping total work linear.
            let share = (self.width / columns.max(1)).max(1);
            natural.fill(share);
            minword.fill(1);
        } else {
            let snapshot = self.snapshot();
            for (rows, layout) in [
                (&head, &head_layout),
                (&body, &body_layout),
                (&foot, &foot_layout),
            ] {
                self.measure_grid(rows, layout, &mut natural, &mut minword);
            }
            self.restore(snapshot);
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
            self.width,
            self.wrap,
        );
        let col_widths: Vec<usize> = content.iter().map(|width| width + 2).collect();
        let head_grid = self.grid_rows(&head, &head_layout, &content);
        let body_grid = self.grid_rows(&body, &body_layout, &content);
        let foot_grid = self.grid_rows(&foot, &foot_layout, &content);

        grid::render(&grid::GridTable {
            col_widths,
            aligns: None,
            head: head_grid,
            body: body_grid,
            foot: foot_grid,
        })
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
                let lines = self.cell_lines(&cell.content, SIMPLE_WIDTH);
                let (width, word) = measure_unbreakable(&lines);
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
        let was_in_cell = self.in_cell;
        self.in_cell = true;
        let text = self.blocks_to_string(content, width, false);
        self.in_cell = was_in_cell;
        if text.is_empty() {
            Vec::new()
        } else {
            text.split('\n').map(str::to_owned).collect()
        }
    }

    /// The `.. table::` directive carrying the caption, or `None` when the caption is empty. Each
    /// caption block contributes one line; the table's own attributes are not emitted.
    fn table_caption(&mut self, caption: &Caption, _width: usize) -> Option<String> {
        let parts: Vec<String> = caption
            .long
            .iter()
            .map(|block| self.flat(block_inlines(block)))
            .filter(|line| !line.is_empty())
            .collect();
        if parts.is_empty() {
            None
        } else {
            Some(format!(".. table:: {}", parts.join("\n")))
        }
    }

    fn snapshot(&self) -> (usize, usize, usize, usize) {
        (
            self.footnotes.len(),
            self.substitutions.len(),
            self.fallback_count,
            self.used_names.len(),
        )
    }

    fn restore(&mut self, snapshot: (usize, usize, usize, usize)) {
        self.footnotes.truncate(snapshot.0);
        self.substitutions.truncate(snapshot.1);
        self.fallback_count = snapshot.2;
        self.used_names.truncate(snapshot.3);
    }
}

/// Width used to render a simple-table cell: large enough that its content never wraps, so a
/// column's width is the natural extent of its widest cell.
const SIMPLE_WIDTH: usize = 100_000;

/// Whether a cell's sole paragraph closes with a space inline, which a simple table keeps as part of
/// the column rather than trimming.
fn cell_ends_with_space(content: &[Block]) -> bool {
    matches!(
        content,
        [Block::Plain(inlines) | Block::Para(inlines)]
            if matches!(inlines.last(), Some(Inline::Space))
    )
}

/// A simple table's `=` rule: a run of `=` per column width, joined by single spaces.
fn equals_rule(widths: &[usize]) -> String {
    widths
        .iter()
        .map(|width| "=".repeat(*width))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The widest line and widest token across rendered lines, where a space escaped by a preceding
/// backslash holds its token together (RST writes a non-breaking space as `\ `, which must not be
/// counted as a column-shrinking break point).
fn measure_unbreakable(lines: &[String]) -> (usize, usize) {
    let mut natural = 0usize;
    let mut minword = 0usize;
    for line in lines {
        natural = natural.max(display_width(line));
        let mut token = String::new();
        let mut prev = '\0';
        for ch in line.chars() {
            if ch.is_whitespace() && prev != '\\' {
                minword = minword.max(display_width(&token));
                token.clear();
            } else {
                token.push(ch);
            }
            prev = ch;
        }
        minword = minword.max(display_width(&token));
    }
    (natural, minword)
}

/// Lay one line of a simple-table row across the column fields. Each column is padded to its width
/// except the last, which is left bare; the columns are joined by single spaces. A spanning cell's
/// content occupies its first column and the columns it covers contribute empty fields.
fn lay_simple_line(
    col_lines: &[Vec<String>],
    widths: &[usize],
    columns: usize,
    line: usize,
) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(columns);
    for col in 0..columns {
        let text = col_lines
            .get(col)
            .and_then(|cell| cell.get(line))
            .map_or("", String::as_str);
        if col + 1 == columns {
            parts.push(text.to_owned());
        } else {
            let width = widths.get(col).copied().unwrap_or(0);
            parts.push(pad_right(text, width));
        }
    }
    parts.join(" ")
}

/// The `-` underline placed beneath a simple-table row that carries a column span: a dash run per
/// cell sized to the merged field it occupies, joined by single spaces.
fn colspan_underline(placements: &[(usize, usize)], widths: &[usize]) -> String {
    placements
        .iter()
        .map(|&(start, span)| {
            let merged =
                widths.iter().skip(start).take(span).sum::<usize>() + span.saturating_sub(1);
            "-".repeat(merged)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Pad `text` on the right with spaces to `width` display columns, leaving content wider than the
/// field untouched.
fn pad_right(text: &str, width: usize) -> String {
    let pad = width.saturating_sub(display_width(text));
    format!("{text}{}", " ".repeat(pad))
}
