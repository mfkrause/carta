//! Table rendering: grid, rows, cells and merged-cell layout.

use super::figures::{Numbered, render_caption};
use super::{
    Ctx, Inherited, Item, LIST_INDENT_STEP, TABLE_TEXT_WIDTH, blocks_plain_text, close_bookmark,
    open_bookmark, paragraph_props, render_blocks,
};
use carta_ast::{Alignment, Cell, ColSpec, ColWidth, Row, Table};
use carta_core::container::xml::Element;

/// Renders a table: its caption paragraphs, then the grid itself with header rows marked and merged
/// cells laid out. A table inside a list item is shifted to align under its item's text and uses up
/// the item's marker; its caption and cells bind to no list number.
pub(super) fn render_table(table: &Table, out: &mut Element, ctx: &mut Ctx, inherited: Inherited) {
    let columns = table.col_specs.len();
    let has_head = !table.head.rows.is_empty();
    let has_foot = !table.foot.rows.is_empty();

    let mark = open_bookmark(table.attr.id.as_str(), out, ctx);
    let item = ctx.item.take();
    let indent = item.map(|item| item.depth);
    ctx.prev_paragraph = false;
    render_caption(
        &table.caption.long,
        out,
        ctx,
        inherited.styled("TableCaption"),
        Numbered::Table,
    );
    ctx.in_table = true;

    let mut tbl = Element::new("w:tbl");
    tbl.push(table_properties(table, has_head, has_foot, indent));
    tbl.push(table_grid(&table.col_specs));

    let mut carried = vec![0u32; columns];
    let mut carried_span = vec![1u32; columns];

    let head = table.head.rows.iter().map(|row| (row, true));
    let bodies = table
        .bodies
        .iter()
        .flat_map(|section| section.head.iter().chain(section.body.iter()));
    let rest = bodies.chain(&table.foot.rows).map(|row| (row, false));
    // A table without columns has no cell to hold content, so it has no rows either.
    for (row, is_header) in head.chain(rest).filter(|_| columns > 0) {
        tbl.push(render_row(
            row,
            &table.col_specs,
            &mut carried,
            &mut carried_span,
            is_header,
            ctx,
            inherited,
        ));
    }
    ctx.in_table = false;
    ctx.item = item.map(|item| Item {
        lead_used: true,
        ..item
    });
    out.push(tbl);
    close_bookmark(mark, out, ctx);
}

/// The table's properties: its style, width, list indent, header/footer look and caption text.
fn table_properties(table: &Table, has_head: bool, has_foot: bool, indent: Option<u32>) -> Element {
    let mut properties = Element::new("w:tblPr");
    properties.push(Element::new("w:tblStyle").attr("w:val", "Table"));

    let sized: f64 = table
        .col_specs
        .iter()
        .filter_map(|spec| match &spec.width {
            ColWidth::ColWidth(fraction) => Some(*fraction),
            ColWidth::ColWidthDefault => None,
        })
        .sum();
    if sized > 0.0 {
        let percent = (sized * 5000.0).round();
        properties.push(
            Element::new("w:tblW")
                .attr("w:type", "pct")
                .attr("w:w", &percent.to_string()),
        );
        properties.push(Element::new("w:tblLayout").attr("w:type", "fixed"));
    } else {
        properties.push(
            Element::new("w:tblW")
                .attr("w:type", "auto")
                .attr("w:w", "0"),
        );
    }

    if let Some(depth) = indent {
        properties.push(Element::new("w:jc").attr("w:val", "left"));
        properties.push(
            Element::new("w:tblInd")
                .attr("w:w", &(LIST_INDENT_STEP * (depth + 1)).to_string())
                .attr("w:type", "dxa"),
        );
    }

    let (first_row, look) = if has_head {
        ("1", "0020")
    } else {
        ("0", "0000")
    };
    let last_row = if has_foot { "1" } else { "0" };
    properties.push(
        Element::new("w:tblLook")
            .attr("w:firstRow", first_row)
            .attr("w:lastRow", last_row)
            .attr("w:firstColumn", "0")
            .attr("w:lastColumn", "0")
            .attr("w:noHBand", "0")
            .attr("w:noVBand", "0")
            .attr("w:val", look),
    );

    let caption = blocks_plain_text(&table.caption.long);
    if !caption.is_empty() {
        properties.push(Element::new("w:tblCaption").attr("w:val", &caption));
    }
    properties
}

/// The column grid: each column's width from its fraction of the text width, or an equal share when
/// no fractions are given.
#[allow(clippy::cast_precision_loss)] // Column counts are tiny, far inside f64's exact range.
fn table_grid(col_specs: &[ColSpec]) -> Element {
    let mut grid = Element::new("w:tblGrid");
    let columns = col_specs.len();
    for spec in col_specs {
        let width = match &spec.width {
            ColWidth::ColWidth(fraction) => (fraction * TABLE_TEXT_WIDTH).round(),
            ColWidth::ColWidthDefault if columns > 0 => (TABLE_TEXT_WIDTH / columns as f64).round(),
            ColWidth::ColWidthDefault => 0.0,
        };
        grid.push(Element::new("w:gridCol").attr("w:w", &width.to_string()));
    }
    grid
}

/// Renders one table row, filling merge-continuation cells for any row-spans carried down from above
/// before laying out the row's own cells. A row with too few cells is completed with empty ones.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Column spans are small counts.
fn render_row(
    row: &Row,
    col_specs: &[ColSpec],
    carried: &mut [u32],
    carried_span: &mut [u32],
    is_header: bool,
    ctx: &mut Ctx,
    inherited: Inherited,
) -> Element {
    let columns = col_specs.len();
    let mut tr = Element::new("w:tr");
    if is_header {
        tr.push(Element::new("w:trPr").child(Element::new("w:tblHeader").attr("w:val", "on")));
    }

    let mut column = 0usize;
    let mut cells = row.cells.iter();
    while column < columns {
        if carried.get(column).copied().unwrap_or(0) > 0 {
            let span = carried_span.get(column).copied().unwrap_or(1).max(1) as usize;
            tr.push(continuation_cell(span as u32));
            decrement_carried(carried, column, span, columns);
            column = column.saturating_add(span).max(column + 1);
            continue;
        }
        let Some(cell) = cells.next() else {
            let mut tc = Element::new("w:tc").child(Element::new("w:tcPr"));
            tc.push(empty_cell_paragraph(ctx));
            tr.push(tc);
            column += 1;
            continue;
        };
        let span = cell.col_span.max(1) as usize;
        let rows = cell.row_span.max(1);
        let jc = effective_jc(cell, col_specs, column).or(inherited.jc);
        let in_cell = Inherited {
            jc,
            ..inherited.nested()
        };
        tr.push(render_normal_cell(cell, span as u32, rows, ctx, in_cell));
        if rows > 1 {
            let remaining = (rows - 1).max(0) as u32;
            let end = column.saturating_add(span).min(columns);
            for slot in column..end {
                if let Some(value) = carried.get_mut(slot) {
                    *value = remaining;
                }
                if let Some(value) = carried_span.get_mut(slot) {
                    *value = span.max(1) as u32;
                }
            }
        }
        column = column.saturating_add(span).max(column + 1);
    }
    tr
}

/// Decrements the carried row-span count for the columns a continuation cell just covered.
fn decrement_carried(carried: &mut [u32], column: usize, span: usize, columns: usize) {
    let end = column.saturating_add(span).min(columns);
    for slot in column..end {
        if let Some(value) = carried.get_mut(slot) {
            *value = value.saturating_sub(1);
        }
    }
}

/// A merge-continuation cell: an empty cell that continues the vertical merge above it, always
/// carrying an explicit grid span so it lines up under the cell it continues.
fn continuation_cell(span: u32) -> Element {
    let properties = Element::new("w:tcPr")
        .child(Element::new("w:gridSpan").attr("w:val", &span.to_string()))
        .child(Element::new("w:vMerge").attr("w:val", "continue"));
    Element::new("w:tc")
        .child(properties)
        .child(Element::new("w:p").child(Element::new("w:pPr")))
}

/// Renders a cell that begins its own content: its grid span and merge start when it spans, then its
/// block content laid out under the cell's alignment.
fn render_normal_cell(
    cell: &Cell,
    span: u32,
    rows: i64,
    ctx: &mut Ctx,
    inherited: Inherited,
) -> Element {
    let mut tc = Element::new("w:tc");
    let mut properties = Element::new("w:tcPr");
    if span > 1 {
        properties.push(Element::new("w:gridSpan").attr("w:val", &span.to_string()));
    }
    if rows > 1 {
        properties.push(Element::new("w:vMerge").attr("w:val", "restart"));
    }
    tc.push(properties);

    let before = tc.child_count();
    render_blocks(&cell.content, &mut tc, ctx, inherited);
    // Cells require a paragraph and must not end on a table; add filler as needed.
    if tc.child_count() == before {
        if cell.content.is_empty() {
            tc.push(empty_cell_paragraph(ctx));
        } else {
            tc.push(Element::new("w:p"));
        }
    } else if tc.last_child_element_name() == Some("w:tbl") {
        tc.push(Element::new("w:p"));
    }
    tc
}

/// The compact, empty paragraph a cell without content holds.
fn empty_cell_paragraph(ctx: &Ctx) -> Element {
    Element::new("w:p").child(paragraph_props(
        Some("Compact"),
        None,
        None,
        ctx.run_properties.right_to_left,
    ))
}

/// The cell's effective horizontal alignment: its own if set, otherwise its column's.
fn effective_jc(cell: &Cell, col_specs: &[ColSpec], column: usize) -> Option<&'static str> {
    let align = match cell.align {
        Alignment::AlignDefault => col_specs.get(column).map(|spec| &spec.align),
        ref own => Some(own),
    };
    match align {
        Some(Alignment::AlignLeft) => Some("left"),
        Some(Alignment::AlignRight) => Some("right"),
        Some(Alignment::AlignCenter) => Some("center"),
        _ => None,
    }
}
