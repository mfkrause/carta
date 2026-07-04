//! The row-span grid model and cell/row accessors shared by the table-rendering writers. The grid
//! model and the row accessors serve two disjoint sets of writers, so which items are live depends
//! on the enabled features; unused-item warnings are allowed here rather than gated per item.
#![allow(dead_code)]

use carta_ast::{Block, Inline};

/// One column slot of a laid-out row: the start of a cell, or a column covered by a column or row
/// span (or a column the row's cells never reached). A consumer renders a covered slot as its own
/// filler placeholder.
pub(crate) enum GridSlot<'cell> {
    Cell(usize, &'cell carta_ast::Cell),
    Covered,
}

/// Resolves each table cell's true starting column within one row group, accounting for cells from
/// earlier rows that still cover columns through their row span. Create one tracker per group of
/// rows a span can extend over (a table head, a body's own head rows, a body's rows, a foot).
#[derive(Debug)]
pub(crate) struct RowSpanGrid {
    /// Per column, how many upcoming rows a span opened in an earlier row still covers.
    pending: Vec<i32>,
}

impl RowSpanGrid {
    pub(crate) fn new(columns: usize) -> Self {
        Self {
            pending: vec![0; columns],
        }
    }

    /// Place one row's cells: each cell lands on the first column not covered from above and
    /// occupies its column span, and its row span is recorded for the rows that follow. Returns
    /// each cell paired with its starting column.
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
            let col_span = usize::try_from(cell.col_span).unwrap_or(1).max(1);
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
pub(crate) fn block_inlines(block: &Block) -> &[Inline] {
    match block {
        Block::Plain(inlines) | Block::Para(inlines) => inlines,
        _ => &[],
    }
}

/// Every row of every body, intermediate head rows included, in document order.
pub(crate) fn body_rows(table: &carta_ast::Table) -> Vec<&carta_ast::Row> {
    table
        .bodies
        .iter()
        .flat_map(|body| body.head.iter().chain(body.body.iter()))
        .collect()
}
