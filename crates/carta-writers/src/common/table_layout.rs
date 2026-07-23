//! Text-grid table layout for the plain and markdown-family writers: form selection, cell padding,
//! row assembly, and piece measurement. The whole module is gated to those writers at its `mod`
//! declaration, so within its feature family every item is referenced.

use super::{Piece, body_rows, display_width, fill_lines};
use carta_ast::{Alignment, Block, Cell, ColWidth, Inline, Row, Table};

/// Width used to render a grid cell when measuring its natural extent, before column widths are
/// fixed: large enough that no reflow occurs.
pub(crate) const MEASURE_WIDTH: usize = 100_000;

/// The layout a text-grid table takes: the compact space-aligned simple form, the reflowing
/// multiline form, or the bordered grid form.
#[derive(Clone, Copy)]
pub(crate) enum TableForm {
    Simple,
    Multiline,
    Grid,
}

/// Choose the rendering form for a table. Spans, block-level cell content, or a footer demand a
/// grid; an explicit column width or a forced break within a cell demands the multiline form;
/// otherwise the compact simple form suffices.
pub(crate) fn table_form(table: &Table) -> TableForm {
    let rows: Vec<&Row> = table
        .head
        .rows
        .iter()
        .chain(body_rows(table))
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
pub(crate) fn is_simple_cell(cell: &Cell) -> bool {
    matches!(
        cell.content.as_slice(),
        [] | [Block::Plain(_) | Block::Para(_)]
    )
}

/// The inline content of a simple cell, or an empty slice for anything richer.
pub(crate) fn cell_inlines(cell: &Cell) -> &[Inline] {
    match cell.content.first() {
        Some(Block::Plain(inlines) | Block::Para(inlines)) => inlines,
        _ => &[],
    }
}

/// A simple cell's inline content with a single leading and/or trailing `Inline::Space` removed —
/// the boundary spaces a table cell does not render. Interior spacing is untouched. Non-simple
/// cells yield an empty slice, as with [`cell_inlines`]. Width and layout math must keep using
/// [`cell_inlines`]; this variant is for render sites only.
pub(crate) fn trimmed_cell_inlines(cell: &Cell) -> &[Inline] {
    let mut inlines = cell_inlines(cell);
    if let [Inline::Space, rest @ ..] = inlines {
        inlines = rest;
    }
    if let [rest @ .., Inline::Space] = inlines {
        inlines = rest;
    }
    inlines
}

/// How many boundary spaces [`trimmed_cell_inlines`] removes from a cell — the width its column
/// still reserves for them even though they are not rendered.
pub(crate) fn boundary_space_count(cell: &Cell) -> usize {
    cell_inlines(cell).len() - trimmed_cell_inlines(cell).len()
}

/// Whether a simple cell contains a forced line break, which forces the multiline form.
pub(crate) fn cell_has_break(cell: &Cell) -> bool {
    is_simple_cell(cell)
        && cell_inlines(cell)
            .iter()
            .any(|inline| matches!(inline, Inline::LineBreak))
}

/// A row of column underlines: a run of dashes per column width, joined by single spaces.
pub(crate) fn dash_rule(field: &[usize]) -> String {
    field
        .iter()
        .map(|width| "-".repeat(*width))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Pad `text` to `width`, placing the slack according to the column's alignment.
pub(crate) fn pad_align(text: &str, width: usize, align: &Alignment) -> String {
    let pad = width.saturating_sub(display_width(text));
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
pub(crate) fn lay_row(
    cells: &[Vec<String>],
    field: &[usize],
    aligns: &[&Alignment],
) -> Vec<String> {
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
pub(crate) fn filled_cells(row: &[Vec<Piece>], field: &[usize]) -> Vec<Vec<String>> {
    row.iter()
        .enumerate()
        .map(|(index, pieces)| {
            let width = field.get(index).copied().unwrap_or(0);
            // A cell always reflows to its computed column width: the width is a layout constraint of
            // the table, not a paragraph wrap the document option can switch off.
            fill_lines(pieces, width)
        })
        .collect()
}

/// Append the body rows of a multiline table, separating rows with a blank line. A lone row still
/// gets a trailing blank to keep it visually distinct from the closing rule.
pub(crate) fn extend_multiline_body(
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
pub(crate) fn indent_lines(lines: &[String], indent: usize) -> String {
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
pub(crate) fn measure_pieces(pieces: &[Piece]) -> (usize, usize) {
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
            Piece::Space | Piece::Soft => {
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
pub(crate) fn pieces_nonempty(pieces: &[Piece]) -> bool {
    pieces
        .iter()
        .any(|piece| matches!(piece, Piece::Text(text) if !text.is_empty()))
}
