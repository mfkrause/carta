//! Table row assembly for the docx reader, resolving vertical merges and column alignment.

use carta_ast::{Alignment, Attr, Block, Cell, Row};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum VMerge {
    Restart,
    Continue,
}

pub(super) struct CellRaw {
    pub(super) start_col: usize,
    pub(super) span: usize,
    pub(super) vmerge: Option<VMerge>,
    pub(super) align: Alignment,
    pub(super) content: Vec<Block>,
}

pub(super) struct RowRaw {
    pub(super) header: bool,
    pub(super) cells: Vec<CellRaw>,
}

/// Resolves vertical merges into row spans and drops the continuation cells they absorb.
pub(super) fn build_rows(rows: Vec<RowRaw>) -> Vec<Row> {
    // First pass resolves each cell's span from the light merge markers (`None` marks a dropped
    // continuation); the second can then move the heavy cell content instead of cloning it.
    let spans: Vec<Vec<Option<i32>>> = rows
        .iter()
        .enumerate()
        .map(|(row_index, row)| {
            row.cells
                .iter()
                .map(|cell| match cell.vmerge {
                    Some(VMerge::Continue) => None,
                    Some(VMerge::Restart) => {
                        let mut span = 1;
                        for below in rows.iter().skip(row_index + 1) {
                            if below.cells.iter().any(|other| {
                                other.start_col == cell.start_col
                                    && other.vmerge == Some(VMerge::Continue)
                            }) {
                                span += 1;
                            } else {
                                break;
                            }
                        }
                        Some(span)
                    }
                    None => Some(1),
                })
                .collect()
        })
        .collect();
    rows.into_iter()
        .zip(spans)
        .map(|(row, row_spans)| {
            let cells = row
                .cells
                .into_iter()
                .zip(row_spans)
                .filter_map(|(cell, span)| {
                    span.map(|row_span| Cell {
                        attr: Attr::default(),
                        align: cell.align,
                        row_span,
                        col_span: i32::try_from(cell.span).unwrap_or(1).max(1),
                        content: cell.content,
                    })
                })
                .collect();
            Row {
                attr: Attr::default(),
                cells,
            }
        })
        .collect()
}

/// The alignment shared by a column, taken from the first body-row cell that begins in it.
pub(super) fn column_alignment(rows: &[RowRaw], head_count: usize, column: usize) -> Alignment {
    for row in rows.iter().skip(head_count) {
        for cell in &row.cells {
            if cell.start_col == column && cell.vmerge != Some(VMerge::Continue) {
                return cell.align.clone();
            }
        }
    }
    Alignment::AlignDefault
}
