//! Text-grid table layout. Renders rows of pre-rendered cell lines into a bordered grid:
//! `-` separators between rows, `=` separators at section boundaries, row and column spans
//! drawn as merged rectangles, and optional alignment colons.

use carta_ast::{Alignment, ColSpec, ColWidth, Row};
use carta_core::WrapMode;

use crate::common::display_width;

/// A cell span (`row_span`/`col_span`) as a count: never below one, never negative.
pub(crate) fn span_count(value: i32) -> usize {
    usize::try_from(value.max(1)).unwrap_or(1)
}

/// The deepest table nesting whose columns are still sized by rendering their cells. Each sizing
/// pass re-renders a cell's content, so nested tables multiply the passes of every ancestor;
/// beyond this depth columns fall back to an even share of the fill width so a deeply nested
/// document renders in linear time.
pub(crate) const MAX_MEASURED_TABLE_NESTING: usize = 6;

/// One table cell: its content already rendered to lines, plus the rectangle of columns and
/// rows it covers. Spans below 1 are treated as 1.
pub(crate) struct GridCell {
    pub(crate) lines: Vec<String>,
    pub(crate) row_span: usize,
    pub(crate) col_span: usize,
}

pub(crate) struct GridRow {
    pub(crate) cells: Vec<GridCell>,
}

pub(crate) struct GridTable<'a> {
    /// Border-segment width of each column: the cell content area plus one padding space on
    /// each side. Every output line has display width `sum(col_widths) + columns + 1`.
    pub(crate) col_widths: Vec<usize>,
    /// Column alignments marked with colons on the separator that closes the head (or on the
    /// top border when there is no head). `None` renders no alignment marks.
    pub(crate) aligns: Option<&'a [Alignment]>,
    pub(crate) head: Vec<GridRow>,
    pub(crate) body: Vec<GridRow>,
    pub(crate) foot: Vec<GridRow>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Head,
    Body,
    Foot,
}

/// A cell pinned to its rectangle: rows `start_row..start_row + row_span` over columns
/// `start_col..start_col + col_span`.
struct PlacedCell {
    start_row: usize,
    start_col: usize,
    col_span: usize,
    lines: Vec<String>,
}

struct Layout {
    placed: Vec<PlacedCell>,
    /// `occupancy[row][col]` is the index into `placed` of the cell covering that position.
    occupancy: Vec<Vec<usize>>,
    sections: Vec<Section>,
    row_heights: Vec<usize>,
}

/// Render the table as grid lines joined by `\n`, with no trailing newline. An empty table
/// (no columns or no rows) renders as an empty string.
pub(crate) fn render(table: &GridTable) -> String {
    let columns = table.col_widths.len();
    let row_count = table.head.len() + table.body.len() + table.foot.len();
    if columns == 0 || row_count == 0 {
        return String::new();
    }

    let layout = place(table, columns);
    let mut lines = Vec::new();
    for row in 0..layout.sections.len() {
        lines.push(separator(table, &layout, row));
        let height = layout.row_heights.get(row).copied().unwrap_or(1);
        for line_index in 0..height {
            lines.push(content_line(table, &layout, row, line_index, columns));
        }
    }
    lines.push(separator(table, &layout, layout.sections.len()));
    lines.join("\n")
}

fn place(table: &GridTable, columns: usize) -> Layout {
    let mut placed: Vec<PlacedCell> = Vec::new();
    let mut occupancy: Vec<Vec<Option<usize>>> = Vec::new();
    let mut sections = Vec::new();

    let groups = [
        (Section::Head, &table.head),
        (Section::Body, &table.body),
        (Section::Foot, &table.foot),
    ];
    for (section, rows) in groups {
        let section_start = occupancy.len();
        for _ in 0..rows.len() {
            occupancy.push(vec![None; columns]);
            sections.push(section);
        }
        for (offset, row) in rows.iter().enumerate() {
            let row_index = section_start + offset;
            let mut col = 0;
            for cell in &row.cells {
                while col < columns
                    && occupancy
                        .get(row_index)
                        .and_then(|slots| slots.get(col))
                        .is_some_and(Option::is_some)
                {
                    col += 1;
                }
                if col >= columns {
                    break;
                }
                let col_span = cell.col_span.max(1).min(columns - col);
                // A span may not cross its section boundary.
                let row_span = cell.row_span.max(1).min(rows.len() - offset);
                let id = placed.len();
                placed.push(PlacedCell {
                    start_row: row_index,
                    start_col: col,
                    col_span,
                    lines: cell.lines.clone(),
                });
                for covered_row in row_index..row_index + row_span {
                    for covered_col in col..col + col_span {
                        if let Some(slot) = occupancy
                            .get_mut(covered_row)
                            .and_then(|slots| slots.get_mut(covered_col))
                        {
                            *slot = Some(id);
                        }
                    }
                }
                col += col_span;
            }
        }
    }

    // A position no cell covers becomes a blank single cell of its own.
    for row in 0..occupancy.len() {
        for col in 0..columns {
            let vacant = occupancy
                .get(row)
                .and_then(|slots| slots.get(col))
                .is_some_and(Option::is_none);
            if vacant {
                let id = placed.len();
                placed.push(PlacedCell {
                    start_row: row,
                    start_col: col,
                    col_span: 1,
                    lines: Vec::new(),
                });
                if let Some(slot) = occupancy.get_mut(row).and_then(|slots| slots.get_mut(col)) {
                    *slot = Some(id);
                }
            }
        }
    }

    let occupancy: Vec<Vec<usize>> = occupancy
        .into_iter()
        .map(|slots| slots.into_iter().map(|slot| slot.unwrap_or(0)).collect())
        .collect();

    let mut row_heights = vec![1; sections.len()];
    for cell in &placed {
        if let Some(height) = row_heights.get_mut(cell.start_row) {
            *height = (*height).max(cell.lines.len());
        }
    }

    Layout {
        placed,
        occupancy,
        sections,
        row_heights,
    }
}

/// For each column, whether boundary `boundary` draws a horizontal rule there: it does unless a
/// row span crosses the boundary in that column, letting the cell continue uninterrupted.
fn drawn_columns(layout: &Layout, boundary: usize, row_count: usize, columns: usize) -> Vec<bool> {
    (0..columns)
        .map(|col| {
            !(boundary > 0
                && boundary < row_count
                && layout
                    .occupancy
                    .get(boundary - 1)
                    .and_then(|slots| slots.get(col))
                    == layout
                        .occupancy
                        .get(boundary)
                        .and_then(|slots| slots.get(col)))
        })
        .collect()
}

fn separator(table: &GridTable, layout: &Layout, boundary: usize) -> String {
    let row_count = layout.sections.len();
    let columns = table.col_widths.len();
    let section_above = boundary.checked_sub(1).and_then(|r| layout.sections.get(r));
    let section_below = layout.sections.get(boundary);
    // Arms stay separate per boundary kind (top / intra-section / inter-section / bottom).
    #[allow(clippy::match_same_arms)]
    let fill = match (section_above, section_below) {
        (None, _) => '-',
        (Some(above), Some(below)) if above == below => '-',
        (Some(_), Some(_)) => '=',
        (Some(Section::Body), None) => '-',
        (Some(_), None) => '=',
    };
    let marks_alignment = match (section_above, section_below) {
        // The boundary that closes the head carries the marks; with no head, the top border.
        (Some(Section::Head), Some(below)) => below != &Section::Head,
        (None, _) => table.head.is_empty(),
        _ => false,
    };

    let drawn = drawn_columns(layout, boundary, row_count, columns);

    let cell_at = |row: usize, col: usize| {
        layout
            .occupancy
            .get(row)
            .and_then(|slots| slots.get(col))
            .copied()
    };
    let exposes = |row: usize, col: usize| cell_at(row, col - 1) != cell_at(row, col);
    // A section-leading separator divides wherever any row of the section below splits the column;
    // an interior boundary uses only the row immediately below.
    let leads_section = boundary < row_count
        && (boundary == 0 || layout.sections.get(boundary - 1) != layout.sections.get(boundary));
    // A divider passes between `col - 1` and `col` when the flanking cells differ above or below,
    // or when an adjacent alignment colon must not be swallowed by a merged run.
    let divider = |col: usize| -> bool {
        let aligned = marks_alignment
            && table.aligns.is_some_and(|aligns| {
                aligns.get(col - 1).is_some_and(marks_right)
                    || aligns.get(col).is_some_and(marks_left)
            });
        let below = if leads_section {
            let section = layout.sections.get(boundary).copied();
            (boundary..row_count)
                .take_while(|&row| layout.sections.get(row).copied() == section)
                .any(|row| exposes(row, col))
        } else {
            boundary < row_count && exposes(boundary, col)
        };
        aligned || (boundary > 0 && exposes(boundary - 1, col)) || below
    };
    let junction = |col: usize| -> char {
        if col == 0 {
            return if drawn.first().copied().unwrap_or(false) {
                '+'
            } else {
                '|'
            };
        }
        if col == columns {
            return if drawn.get(columns - 1).copied().unwrap_or(false) {
                '+'
            } else {
                '|'
            };
        }
        let horizontal = drawn.get(col - 1).copied().unwrap_or(false)
            || drawn.get(col).copied().unwrap_or(false);
        match (horizontal, divider(col)) {
            (true, true) => '+',
            (false, true) => '|',
            (true, false) => fill,
            (false, false) => ' ',
        }
    };

    let mut line = String::new();
    for (col, &width) in table.col_widths.iter().enumerate() {
        line.push(junction(col));
        if drawn.get(col).copied().unwrap_or(false) {
            let align = if marks_alignment {
                table
                    .aligns
                    .and_then(|aligns| aligns.get(col))
                    .cloned()
                    .unwrap_or(Alignment::AlignDefault)
            } else {
                Alignment::AlignDefault
            };
            line.push_str(&segment(fill, width, &align));
        } else {
            line.extend(std::iter::repeat_n(' ', width));
        }
    }
    line.push(junction(columns));
    line
}

/// Whether an alignment places a colon on the left edge of its column's separator segment.
fn marks_left(align: &Alignment) -> bool {
    matches!(align, Alignment::AlignLeft | Alignment::AlignCenter)
}

/// Whether an alignment places a colon on the right edge of its column's separator segment.
fn marks_right(align: &Alignment) -> bool {
    matches!(align, Alignment::AlignRight | Alignment::AlignCenter)
}

/// A column's stretch of a separator line, with alignment colons replacing the first and/or
/// last fill character.
fn segment(fill: char, width: usize, align: &Alignment) -> String {
    let mut chars: Vec<char> = std::iter::repeat_n(fill, width).collect();
    let (left, right) = match align {
        Alignment::AlignLeft => (true, false),
        Alignment::AlignRight => (false, true),
        Alignment::AlignCenter => (true, true),
        Alignment::AlignDefault => (false, false),
    };
    if left && let Some(first) = chars.first_mut() {
        *first = ':';
    }
    if right && let Some(last) = chars.last_mut() {
        *last = ':';
    }
    chars.into_iter().collect()
}

fn content_line(
    table: &GridTable,
    layout: &Layout,
    row: usize,
    line_index: usize,
    columns: usize,
) -> String {
    let mut line = String::new();
    let mut col = 0;
    while col < columns {
        let cell = layout
            .occupancy
            .get(row)
            .and_then(|slots| slots.get(col))
            .and_then(|&id| layout.placed.get(id));
        let Some(cell) = cell else {
            break;
        };
        let span = cell.col_span.max(1);
        let field_width: usize = table
            .col_widths
            .iter()
            .skip(cell.start_col)
            .take(span)
            .sum::<usize>()
            + span.saturating_sub(1);
        // A spanning cell renders all content in its first row; covered rows below show blanks.
        let text = if cell.start_row == row {
            cell.lines.get(line_index).map_or("", String::as_str)
        } else {
            ""
        };
        let padding = field_width
            .saturating_sub(2)
            .saturating_sub(display_width(text));
        line.push_str("| ");
        line.push_str(text);
        line.extend(std::iter::repeat_n(' ', padding));
        line.push(' ');
        col = cell.start_col + span;
    }
    line.push('|');
    line
}

/// Resolve each cell to its starting column and column span, honoring spans already placed in
/// earlier rows via an occupancy matrix.
pub(crate) fn place_columns(rows: &[&Row], columns: usize) -> Vec<Vec<(usize, usize)>> {
    let mut occupied: Vec<Vec<bool>> = (0..rows.len()).map(|_| vec![false; columns]).collect();
    let mut result = Vec::with_capacity(rows.len());
    for (row_index, row) in rows.iter().enumerate() {
        let mut placements = Vec::with_capacity(row.cells.len());
        let mut col = 0usize;
        for cell in &row.cells {
            while col < columns
                && occupied
                    .get(row_index)
                    .and_then(|slots| slots.get(col))
                    .copied()
                    .unwrap_or(false)
            {
                col += 1;
            }
            if col >= columns {
                break;
            }
            let col_span = span_count(cell.col_span).min(columns - col);
            let row_span = span_count(cell.row_span).min(rows.len() - row_index);
            for covered_row in row_index..row_index + row_span {
                for covered_col in col..col + col_span {
                    if let Some(slot) = occupied
                        .get_mut(covered_row)
                        .and_then(|slots| slots.get_mut(covered_col))
                    {
                        *slot = true;
                    }
                }
            }
            placements.push((col, col_span));
            col += col_span;
        }
        result.push(placements);
    }
    result
}

/// The content width of a cell spanning `span` columns from `start`, including the borders absorbed
/// between the merged columns.
pub(crate) fn merged_width(content: &[usize], start: usize, span: usize) -> usize {
    let total: usize = content.iter().skip(start).take(span).sum();
    total + 3 * span.saturating_sub(1)
}

/// The content width a fractional column spec maps to in a grid table, scaled against the fill
/// column.
// `floor` truncation intended; the fraction clamps to the line width so an out-of-range spec
// cannot inflate the column into a huge allocation.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn explicit_grid_width(fraction: f64, width: usize) -> i64 {
    let scaled = (fraction * width as f64).floor().min(width as f64);
    scaled as i64 - 3
}

/// Resolve grid content widths: explicit fractional specs when present, otherwise a
/// content-proportional fit.
// A small width clamped non-negative by `max(0)` converts back to usize exactly.
#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
pub(crate) fn grid_content_widths(
    specs: &[ColSpec],
    natural: &[usize],
    minword: &[usize],
    colspans: &[(usize, usize)],
    columns: usize,
    width: usize,
    wrap: WrapMode,
) -> Vec<usize> {
    let explicit = specs.iter().any(|spec| match &spec.width {
        ColWidth::ColWidth(fraction) => {
            *fraction > 0.0 && explicit_grid_width(*fraction, width) > 0
        }
        ColWidth::ColWidthDefault => false,
    });
    if !explicit {
        return auto_grid_widths(natural, minword, columns, width, wrap);
    }
    let mut widths: Vec<usize> = (0..columns)
        .map(|index| match specs.get(index).map(|spec| &spec.width) {
            Some(ColWidth::ColWidth(fraction)) if *fraction > 0.0 => {
                let scaled = explicit_grid_width(*fraction, width).max(0) as usize;
                scaled.max(minword.get(index).copied().unwrap_or(0))
            }
            _ => natural.get(index).copied().unwrap_or(0),
        })
        .collect();
    for &(start, span) in colspans {
        let floor = colspan_width_floor(specs, start, span, width);
        for column in start..start + span {
            if let Some(value) = widths.get_mut(column) {
                *value = (*value).max(floor);
            }
        }
    }
    widths
}

/// The common width every column spanned by a multi-column cell is widened to.
///
/// A multi-column cell lays its columns out at one shared width. From the combined fractional share
/// of the covered columns, `merged = floor(fraction * total)` gives the character budget for the
/// span; that budget is split evenly, `floor(merged / span)`, with the `merged % span` leftover
/// characters folded back in, and one character per column is held back for the cell's own padding.
// Signed intermediates clamp via `max(0)`; column counts never approach integer limits.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn colspan_width_floor(specs: &[ColSpec], start: usize, span: usize, width: usize) -> usize {
    let span_fraction: f64 = (start..start + span)
        .filter_map(|index| match specs.get(index).map(|spec| &spec.width) {
            Some(ColWidth::ColWidth(fraction)) => Some(*fraction),
            _ => None,
        })
        .sum();
    // Clamp the span budget to the line width; an over-wide sum would widen every column without bound.
    let merged = (span_fraction * width as f64).floor().min(width as f64) as i64;
    let span = span.max(1) as i64;
    (merged / span + merged % span - 1).max(0) as usize
}

/// Distribute the available width across columns: a column narrower than its fair share keeps its
/// natural width and frees the surplus; the rest split what remains, floored at their longest word.
#[allow(
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation
)]
fn auto_grid_widths(
    natural: &[usize],
    minword: &[usize],
    columns: usize,
    width: usize,
    wrap: WrapMode,
) -> Vec<usize> {
    // Without wrapping each column grows to its widest cell instead of sharing a budget.
    if wrap == WrapMode::None {
        return (0..columns)
            .map(|index| natural.get(index).copied().unwrap_or(0))
            .collect();
    }
    let available = (width as i64).saturating_sub(1) - 3 * columns as i64;
    let mut budget = available.max(0) as usize;
    let mut assigned: Vec<Option<usize>> = vec![None; columns];
    let mut remaining: Vec<usize> = (0..columns).collect();
    while !remaining.is_empty() {
        let share = budget / remaining.len();
        let mut fits = vec![false; columns];
        for &index in &remaining {
            if natural.get(index).copied().unwrap_or(0) <= share
                && let Some(slot) = fits.get_mut(index)
            {
                *slot = true;
            }
        }
        if fits.iter().all(|fit| !fit) {
            for &index in &remaining {
                let floor = minword.get(index).copied().unwrap_or(0);
                if let Some(slot) = assigned.get_mut(index) {
                    *slot = Some(share.max(floor));
                }
            }
            break;
        }
        let is_fit = |index: usize| fits.get(index).copied().unwrap_or(false);
        for &index in remaining.iter().filter(|&&index| is_fit(index)) {
            let width = natural.get(index).copied().unwrap_or(0);
            if let Some(slot) = assigned.get_mut(index) {
                *slot = Some(width);
            }
            budget = budget.saturating_sub(width);
        }
        remaining.retain(|&index| !is_fit(index));
    }
    (0..columns)
        .map(|index| {
            assigned
                .get(index)
                .copied()
                .flatten()
                .unwrap_or_else(|| natural.get(index).copied().unwrap_or(0))
        })
        .collect()
}

/// The widest line and widest whitespace-delimited token across a set of rendered lines.
pub(crate) fn measure_lines(lines: &[String]) -> (usize, usize) {
    let mut natural = 0usize;
    let mut minword = 0usize;
    for line in lines {
        natural = natural.max(display_width(line));
        for token in line.split_whitespace() {
            minword = minword.max(display_width(token));
        }
    }
    (natural, minword)
}

#[cfg(test)]
mod tests;
