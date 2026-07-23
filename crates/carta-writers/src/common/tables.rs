//! The row-span grid model and cell/row accessors shared by the table-rendering writers. The grid
//! model and the row accessors serve two disjoint sets of writers.

use carta_ast::{Block, Inline};

/// One column slot of a laid-out row: the start of a cell, or a column covered by a column or row
/// span (or a column the row's cells never reached). A consumer renders a covered slot as its own
/// filler placeholder.
#[cfg_attr(
    not(any(
        feature = "asciidoc",
        feature = "commonmark",
        feature = "dokuwiki",
        feature = "gfm",
        feature = "html",
        feature = "jira",
        feature = "man",
        feature = "markdown",
        feature = "mediawiki",
        feature = "rtf"
    )),
    allow(dead_code)
)]
pub(crate) enum GridSlot<'cell> {
    Cell(usize, &'cell carta_ast::Cell),
    Covered,
}

/// Resolves each table cell's true starting column within one row group, accounting for cells from
/// earlier rows that still cover columns through their row span. Create one tracker per group of
/// rows a span can extend over (a table head, a body's own head rows, a body's rows, a foot).
#[cfg_attr(
    not(any(
        feature = "asciidoc",
        feature = "commonmark",
        feature = "dokuwiki",
        feature = "gfm",
        feature = "html",
        feature = "jira",
        feature = "man",
        feature = "markdown",
        feature = "mediawiki",
        feature = "rtf"
    )),
    allow(dead_code)
)]
#[derive(Debug)]
pub(crate) struct RowSpanGrid {
    /// The table's declared column count; a cell's column span cannot cover columns past it.
    columns: usize,
    /// Per column, how many upcoming rows a span opened in an earlier row still covers.
    pending: Vec<i32>,
}

#[cfg_attr(
    not(any(
        feature = "asciidoc",
        feature = "commonmark",
        feature = "dokuwiki",
        feature = "gfm",
        feature = "html",
        feature = "jira",
        feature = "man",
        feature = "markdown",
        feature = "mediawiki",
        feature = "rtf"
    )),
    allow(dead_code)
)]
impl RowSpanGrid {
    pub(crate) fn new(columns: usize) -> Self {
        Self {
            columns,
            pending: vec![0; columns],
        }
    }

    /// Place one row's cells: each cell lands on the first column not covered from above and
    /// occupies its column span, and its row span is recorded for the rows that follow. Returns
    /// each cell paired with its starting column.
    #[cfg_attr(
        not(any(
            feature = "asciidoc",
            feature = "commonmark",
            feature = "gfm",
            feature = "html",
            feature = "markdown",
            feature = "mediawiki"
        )),
        allow(dead_code)
    )]
    pub(crate) fn place<'cells>(
        &mut self,
        cells: &'cells [carta_ast::Cell],
    ) -> Vec<(usize, &'cells carta_ast::Cell)> {
        self.place_slots(cells)
            .into_iter()
            .filter_map(|slot| match slot {
                GridSlot::Cell(column, cell) => Some((column, cell)),
                GridSlot::Covered => None,
            })
            .collect()
    }

    /// Place one row's cells, surfacing every column slot in order: a cell at its starting column,
    /// or a covered placeholder for a column held by a column span, a row span opened above, or the
    /// trailing columns a row span still holds past the row's own cells. Columns the row never
    /// reached (no span covers them) are not emitted; a consumer that lays out a fixed column count
    /// pads those itself.
    pub(crate) fn place_slots<'cells>(
        &mut self,
        cells: &'cells [carta_ast::Cell],
    ) -> Vec<GridSlot<'cells>> {
        let covered: Vec<usize> = self
            .pending
            .iter()
            .enumerate()
            .filter(|(_, rows)| **rows > 0)
            .map(|(column, _)| column)
            .collect();
        let mut slots: Vec<GridSlot<'cells>> = Vec::with_capacity(cells.len());
        let mut column = 0_usize;
        for cell in cells {
            while self.pending.get(column).copied().unwrap_or(0) > 0 {
                slots.push(GridSlot::Covered);
                column = column.saturating_add(1);
            }
            slots.push(GridSlot::Cell(column, cell));
            // A column span covers real columns only up to the table's own edge; clamping to the
            // columns actually remaining keeps a rogue span value from driving unbounded
            // covered-slot and tracking work.
            let remaining = self.columns.saturating_sub(column).max(1);
            let col_span = usize::try_from(cell.col_span)
                .unwrap_or(1)
                .clamp(1, remaining);
            let end = column.saturating_add(col_span);
            for _ in 1..col_span {
                slots.push(GridSlot::Covered);
            }
            if self.pending.len() < end {
                self.pending.resize(end, 0);
            }
            for slot in self.pending.iter_mut().take(end).skip(column) {
                *slot = cell.row_span.saturating_sub(1).max(0);
            }
            column = end;
        }
        while self.pending.get(column).copied().unwrap_or(0) > 0 {
            slots.push(GridSlot::Covered);
            column = column.saturating_add(1);
        }
        for column in covered {
            if let Some(rows) = self.pending.get_mut(column) {
                *rows -= 1;
            }
        }
        slots
    }
}

/// The inline content of a block, or an empty slice for a block that carries none directly.
#[cfg_attr(
    not(any(
        feature = "gfm",
        feature = "markdown",
        feature = "plain",
        feature = "rst"
    )),
    allow(dead_code)
)]
pub(crate) fn block_inlines(block: &Block) -> &[Inline] {
    match block {
        Block::Plain(inlines) | Block::Para(inlines) => inlines,
        _ => &[],
    }
}

/// Every row of every body, intermediate head rows included, in document order.
#[cfg_attr(
    not(any(
        feature = "gfm",
        feature = "markdown",
        feature = "plain",
        feature = "rst"
    )),
    allow(dead_code)
)]
pub(crate) fn body_rows(table: &carta_ast::Table) -> Vec<&carta_ast::Row> {
    table
        .bodies
        .iter()
        .flat_map(|body| body.head.iter().chain(body.body.iter()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use carta_ast::Cell;

    fn cell(row_span: i32, col_span: i32) -> Cell {
        Cell {
            attr: carta_ast::Attr::default(),
            align: carta_ast::Alignment::AlignDefault,
            row_span,
            col_span,
            content: Vec::new(),
        }
    }

    fn slot_kinds(slots: &[GridSlot]) -> Vec<char> {
        slots
            .iter()
            .map(|slot| match slot {
                GridSlot::Cell(_, _) => 'c',
                GridSlot::Covered => '-',
            })
            .collect()
    }

    #[test]
    fn column_span_within_table_covers_following_columns() {
        let mut grid = RowSpanGrid::new(3);
        let row = [cell(1, 2), cell(1, 1)];
        assert_eq!(slot_kinds(&grid.place_slots(&row)), ['c', '-', 'c']);
    }

    #[test]
    fn column_span_clamps_to_table_edge() {
        let mut grid = RowSpanGrid::new(3);
        let row = [cell(1, 1), cell(1, i32::MAX)];
        assert_eq!(slot_kinds(&grid.place_slots(&row)), ['c', 'c', '-']);
    }

    #[test]
    fn column_span_in_zero_column_table_stays_single() {
        let mut grid = RowSpanGrid::new(0);
        let row = [cell(1, i32::MAX), cell(1, i32::MAX)];
        assert_eq!(slot_kinds(&grid.place_slots(&row)), ['c', 'c']);
    }

    #[test]
    fn nonpositive_spans_occupy_one_column_and_one_row() {
        let mut grid = RowSpanGrid::new(2);
        let first = [cell(0, -5), cell(-1, 0)];
        assert_eq!(slot_kinds(&grid.place_slots(&first)), ['c', 'c']);
        let second = [cell(1, 1), cell(1, 1)];
        assert_eq!(slot_kinds(&grid.place_slots(&second)), ['c', 'c']);
    }

    #[test]
    fn oversized_row_span_covers_each_following_row() {
        let mut grid = RowSpanGrid::new(2);
        let first = [cell(i32::MAX, 1), cell(1, 1)];
        assert_eq!(slot_kinds(&grid.place_slots(&first)), ['c', 'c']);
        let second = [cell(1, 1)];
        assert_eq!(slot_kinds(&grid.place_slots(&second)), ['-', 'c']);
        let third = [cell(1, 1)];
        assert_eq!(slot_kinds(&grid.place_slots(&third)), ['-', 'c']);
    }
}
