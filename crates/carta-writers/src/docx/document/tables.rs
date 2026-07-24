//! Table rendering: grid, rows, cells and merged-cell layout.

use super::figures::{Numbered, render_caption};
use super::runs::code_paragraph;
use super::{
    Ctx, FlowStyle, LIST_INDENT_STEP, TABLE_TEXT_WIDTH, blocks_plain_text, body_style,
    close_bookmark, is_display_equation, open_bookmark, paragraph_props, render_flow,
    separate_adjacent_tables, styled_flow_with_display, styled_paragraph,
};
use carta_ast::{Alignment, Block, Cell, ColSpec, ColWidth, Row, Table};
use carta_core::container::xml::Element;

/// Renders a table: its caption paragraphs, then the grid itself with header rows marked and merged
/// cells laid out. `indent`, when set, is the list-nesting depth a table inside a list item is shifted
/// by, so it aligns under its item's text.
pub(super) fn render_table(table: &Table, out: &mut Element, ctx: &mut Ctx, indent: Option<u32>) {
    let columns = table.col_specs.len();
    let has_head = !table.head.rows.is_empty();
    let has_foot = !table.foot.rows.is_empty();

    let mark = open_bookmark(table.attr.id.as_str(), out, ctx);
    // A table opens a fresh first/body run; a preceding caption takes the opening slot.
    ctx.prev_paragraph = false;
    render_caption(
        &table.caption.long,
        out,
        ctx,
        "TableCaption",
        Numbered::Table,
    );
    if !blocks_plain_text(&table.caption.long).is_empty() {
        ctx.prev_paragraph = true;
    }

    let mut tbl = Element::new("w:tbl");
    tbl.push(table_properties(table, has_head, has_foot, indent));
    tbl.push(table_grid(&table.col_specs));

    // Carried row-spans leave a merge-continuation cell in each row below, at the covered width.
    let mut carried = vec![0u32; columns];
    let mut carried_span = vec![1u32; columns];

    for row in &table.head.rows {
        tbl.push(render_row(
            row,
            &table.col_specs,
            &mut carried,
            &mut carried_span,
            columns,
            true,
            ctx,
        ));
    }
    for section in &table.bodies {
        for row in section.head.iter().chain(section.body.iter()) {
            tbl.push(render_row(
                row,
                &table.col_specs,
                &mut carried,
                &mut carried_span,
                columns,
                false,
                ctx,
            ));
        }
    }
    for row in &table.foot.rows {
        tbl.push(render_row(
            row,
            &table.col_specs,
            &mut carried,
            &mut carried_span,
            columns,
            false,
            ctx,
        ));
    }
    out.push(tbl);
    close_bookmark(mark, out);
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

    // A list-nested table shifts one indent step per level to sit under its item's text.
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
/// before laying out the row's own cells.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Column spans are small counts.
fn render_row(
    row: &Row,
    col_specs: &[ColSpec],
    carried: &mut [u32],
    carried_span: &mut [u32],
    columns: usize,
    is_header: bool,
    ctx: &mut Ctx,
) -> Element {
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
            break;
        };
        let span = cell.col_span.max(1) as usize;
        let rows = cell.row_span.max(1);
        let jc = effective_jc(cell, col_specs, column);
        tr.push(render_normal_cell(cell, span as u32, rows, jc, ctx));
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
/// block content laid out under the cell's effective alignment.
fn render_normal_cell(
    cell: &Cell,
    span: u32,
    rows: i32,
    jc: Option<&str>,
    ctx: &mut Ctx,
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

    let mut wrote = false;
    let mut previous = None;
    for block in &cell.content {
        separate_adjacent_tables(previous, block, &mut tc);
        wrote |= render_cell_block(block, &mut tc, jc, ctx);
        previous = Some(block);
    }
    // A cell must hold a paragraph and must not end on a table: empty content gets a compact
    // filler, content that rendered nothing or ended on a table gets a bare filler paragraph.
    if !wrote {
        if cell.content.is_empty() {
            tc.push(Element::new("w:p").child(paragraph_props(Some("Compact"), None, None)));
        } else {
            tc.push(Element::new("w:p"));
        }
    } else if tc.last_child_element_name() == Some("w:tbl") {
        tc.push(Element::new("w:p"));
    }
    tc
}

/// Renders one block of a cell's content, applying the cell's alignment to its direct paragraphs.
/// Returns whether it emitted anything.
fn render_cell_block(block: &Block, tc: &mut Element, jc: Option<&str>, ctx: &mut Ctx) -> bool {
    match block {
        // Cell paragraphs join the table's first/body run; display equations lift onto their own
        // centred paragraph in the same style.
        Block::Para(inlines) => {
            if inlines.is_empty() && !ctx.features.keep_empty_paragraphs {
                return false;
            }
            let style = body_style(ctx.prev_paragraph);
            let emitted = if inlines.iter().any(is_display_equation) {
                styled_flow_with_display(style, jc, inlines, ctx, tc)
            } else {
                tc.push(styled_paragraph(Some(style), None, jc, inlines, ctx));
                true
            };
            ctx.prev_paragraph = true;
            emitted
        }
        Block::Plain(inlines) => {
            if inlines.is_empty() && !ctx.features.keep_empty_paragraphs {
                return false;
            }
            tc.push(styled_paragraph(Some("Compact"), None, jc, inlines, ctx));
            // A compact paragraph still advances the table's first/body run.
            ctx.prev_paragraph = true;
            true
        }
        Block::CodeBlock(attr, code) => {
            tc.push(code_paragraph(attr, code, None, &ctx.highlighter));
            true
        }
        other => {
            let before = tc.child_count();
            render_flow(
                other,
                tc,
                ctx,
                FlowStyle {
                    para: "BodyText",
                    plain: "Compact",
                    list_ambient: None,
                },
            );
            tc.child_count() > before
        }
    }
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
