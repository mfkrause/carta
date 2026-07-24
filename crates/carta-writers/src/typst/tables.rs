//! Table rendering for the Typst writer.

use std::fmt::Write as _;

use carta_ast::{Alignment, Block, Cell, ColWidth, Inline, Table};
use carta_core::WrapMode;

use crate::common::display_width;

use super::escape::trim_percent;
use super::inline::{fill_cell, fragments, inline_run};
use super::{Fragment, blocks, label};

pub(super) fn render_table(table: &Table, width: usize, wrap: WrapMode, smart: bool) -> String {
    let columns = table_columns(table);
    let aligns = table_aligns(table);
    let mut grid = String::new();
    let _ = writeln!(grid, "    columns: {columns},");
    let _ = writeln!(grid, "    align: {aligns},");

    let head_rows = collect_rows(&table.head.rows);
    if !head_rows.is_empty() {
        let cells: Vec<&Cell> = head_rows.into_iter().flatten().collect();
        let _ = writeln!(
            grid,
            "    table.header({},),",
            render_row(&cells, "    table.header(", ",),", 6, width, wrap, smart)
        );
        grid.push_str("    table.hline(),\n");
    }

    for body in &table.bodies {
        let head = collect_rows(&body.head);
        emit_rows(&mut grid, &head, width, wrap, smart);
        if !head.is_empty() {
            grid.push_str("    table.hline(),\n");
        }
        emit_rows(&mut grid, &collect_rows(&body.body), width, wrap, smart);
    }

    let foot_rows = collect_rows(&table.foot.rows);
    if !foot_rows.is_empty() {
        grid.push_str("    table.hline(),\n");
        let cells: Vec<&Cell> = foot_rows.into_iter().flatten().collect();
        let _ = writeln!(
            grid,
            "    table.footer({},),",
            render_row(&cells, "    table.footer(", ",),", 6, width, wrap, smart)
        );
    }

    let mut out = format!("#figure(\n  align(center)[#table(\n{grid}  )]\n");
    if !table.caption.long.is_empty() {
        let _ = writeln!(
            out,
            "  , caption: {}",
            table_caption(&table.caption.long, width, smart)
        );
    }
    out.push_str("  , kind: table\n  )");
    match label(&table.attr.id) {
        Some(rendered) => format!("{out}\n{rendered}"),
        None => out,
    }
}

/// Render a table caption within `[..]`. A single inline block stays on one line; richer content is
/// laid out as an indented block, two columns in.
fn table_caption(content: &[Block], width: usize, smart: bool) -> String {
    if let [Block::Plain(inlines) | Block::Para(inlines)] = content {
        format!("[{}]", inline_run(inlines, width, WrapMode::Auto, smart))
    } else {
        let mut body = blocks(content, width, WrapMode::Auto, smart);
        if !matches!(content.last(), Some(Block::Plain(_))) {
            body.push('\n');
        }
        format!("[{}\n  ]", indent_continuation(&body, "  "))
    }
}

fn emit_rows(grid: &mut String, rows: &[Vec<&Cell>], width: usize, wrap: WrapMode, smart: bool) {
    for row in rows {
        let _ = writeln!(
            grid,
            "    {},",
            render_row(row, "    ", ",", 4, width, wrap, smart)
        );
    }
}

/// Render a row's cells as a `, `-joined sequence. Each cell's content is laid out from the column
/// where its opening bracket falls (so a long cell wraps against the fill column), which depends on
/// the `prefix` that opens the row line and the widths of the cells before it.
fn render_row(
    row: &[&Cell],
    prefix: &str,
    suffix: &str,
    indent: usize,
    width: usize,
    wrap: WrapMode,
    smart: bool,
) -> String {
    let mut out = String::new();
    let mut column = display_width(prefix);
    for (index, cell) in row.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
            column += 2;
        }
        let glue = trailing_glue_width(row, index, suffix, width, wrap, smart);
        let rendered = table_cell(cell, column, indent, width, wrap, smart, glue);
        match rendered.rfind('\n') {
            Some(position) => column = display_width(&rendered[position + 1..]),
            None => column += display_width(&rendered),
        }
        out.push_str(&rendered);
    }
    out
}

/// The non-breaking text glued after cell `index`'s inline content: its closing bracket, then for each
/// following cell its separator, prefix, opening bracket, and leading run, until a content break point
/// (a space or line break) is reached. After the last cell the row's `suffix` closes the run. A cell's
/// last word and this run share a physical line, so the run's width enters that word's wrap decision.
fn trailing_glue_width(
    row: &[&Cell],
    index: usize,
    suffix: &str,
    width: usize,
    wrap: WrapMode,
    smart: bool,
) -> usize {
    let mut total = 1; // the cell's own closing `]`
    let mut next = index + 1;
    loop {
        let Some(cell) = row.get(next) else {
            total += display_width(suffix);
            return total;
        };
        total += 2 + display_width(&cell_prefix(cell)) + 1; // `, ` + prefix + `[`
        let (run, breaks) = content_leading_run(cell, width, wrap, smart);
        total += run;
        if breaks {
            return total;
        }
        total += 1; // this cell is fully glued; close its `]` and continue
        next += 1;
    }
}

/// A cell's leading content run: the width up to its first break point and whether one exists. Inline
/// content runs to its first space or line break; block content runs to the end of its first physical
/// line. When no break exists the run glues onward into the following cell.
fn content_leading_run(cell: &Cell, width: usize, wrap: WrapMode, smart: bool) -> (usize, bool) {
    match cell.content.as_slice() {
        [] => (0, false),
        [Block::Plain(inlines) | Block::Para(inlines)] => {
            let mut run = 0;
            for fragment in fragments(inlines, width, wrap, smart) {
                match fragment {
                    Fragment::Text(text) | Fragment::Atom(text) => run += display_width(&text),
                    Fragment::Space | Fragment::Soft | Fragment::LineBreak => return (run, true),
                }
            }
            (run, false)
        }
        other => {
            let body = blocks(other, width, wrap, smart);
            let first_line = body.split('\n').next().unwrap_or("");
            (display_width(first_line), body.contains('\n'))
        }
    }
}

/// The `table.cell(..)` wrapper opening a spanning cell, or empty for a plain one. Row spans precede
/// column spans in the argument list.
fn cell_prefix(cell: &Cell) -> String {
    let mut spans = Vec::new();
    if cell.row_span != 1 {
        spans.push(format!("rowspan: {}", cell.row_span));
    }
    if cell.col_span != 1 {
        spans.push(format!("colspan: {}", cell.col_span));
    }
    if spans.is_empty() {
        String::new()
    } else {
        format!("table.cell({})", spans.join(", "))
    }
}

fn collect_rows(rows: &[carta_ast::Row]) -> Vec<Vec<&Cell>> {
    rows.iter().map(|row| row.cells.iter().collect()).collect()
}

/// The `columns:` argument: a bare count when every column takes the default width, or a tuple of
/// percentages when any column carries an explicit fractional width.
fn table_columns(table: &Table) -> String {
    let has_explicit = table
        .col_specs
        .iter()
        .any(|spec| matches!(spec.width, ColWidth::ColWidth(_)));
    if has_explicit {
        let widths: Vec<String> = table
            .col_specs
            .iter()
            .map(|spec| match spec.width {
                ColWidth::ColWidth(fraction) => format!("{}%", trim_percent(fraction * 100.0)),
                ColWidth::ColWidthDefault => "auto".to_owned(),
            })
            .collect();
        format!("({})", widths.join(", "))
    } else {
        table.col_specs.len().to_string()
    }
}

fn table_aligns(table: &Table) -> String {
    let mut out = String::from("(");
    for spec in &table.col_specs {
        out.push_str(alignment(&spec.align));
        out.push(',');
    }
    out.push(')');
    out
}

fn alignment(value: &Alignment) -> &'static str {
    match value {
        Alignment::AlignLeft => "left",
        Alignment::AlignRight => "right",
        Alignment::AlignCenter => "center",
        Alignment::AlignDefault => "auto",
    }
}

fn table_cell(
    cell: &Cell,
    column: usize,
    indent: usize,
    width: usize,
    wrap: WrapMode,
    smart: bool,
    glue: usize,
) -> String {
    let prefix = cell_prefix(cell);
    let bracket_column = column + display_width(&prefix);
    format!(
        "{prefix}{}",
        cell_content(
            &cell.content,
            bracket_column,
            indent,
            width,
            wrap,
            smart,
            glue
        )
    )
}

/// Render a cell's content within `[..]`. A single block of inline content fills against the column
/// where its opening bracket sits; richer content is laid out as an indented block. Wrapped lines sit
/// `indent` columns in.
fn cell_content(
    content: &[Block],
    bracket_column: usize,
    indent: usize,
    width: usize,
    wrap: WrapMode,
    smart: bool,
    glue: usize,
) -> String {
    let pad = " ".repeat(indent);
    match content {
        [Block::Plain(inlines) | Block::Para(inlines)] => {
            let filled = fill_cell(
                &fragments(inlines, width, wrap, smart),
                bracket_column + 1,
                indent,
                width,
                wrap,
                glue,
            );
            // A trailing space inline is cell content the fill drops; restore it before `]`.
            let closing = if matches!(inlines.last(), Some(Inline::Space)) {
                " ]"
            } else {
                "]"
            };
            format!("[{}{closing}", indent_continuation(&filled, &pad))
        }
        [] => "[]".to_owned(),
        blocks_value => {
            let mut body = blocks(blocks_value, width, wrap, smart);
            if !matches!(blocks_value.last(), Some(Block::Plain(_))) {
                body.push('\n');
            }
            format!("[{}\n{pad}]", indent_continuation(&body, &pad))
        }
    }
}

/// Prefix every line after the first with `indent`, leaving blank lines bare.
fn indent_continuation(body: &str, indent: &str) -> String {
    let mut out = String::new();
    for (index, line) in body.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
            if !line.is_empty() {
                out.push_str(indent);
            }
        }
        out.push_str(line);
    }
    out
}
