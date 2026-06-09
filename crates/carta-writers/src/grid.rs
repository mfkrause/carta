//! Text-grid table layout. Renders rows of pre-rendered cell lines into a bordered grid:
//! `-` separators between rows, `=` separators at section boundaries, row and column spans
//! drawn as merged rectangles, and optional alignment colons.

use carta_ast::Alignment;

use crate::common::display_width;

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

/// The border line above row `boundary` (or the bottom border when `boundary` equals the row
/// count). `=` marks a section change and the bottom of a footed table; cells spanning the
/// boundary leave their columns blank, with `+` junctions only next to drawn segments.
fn separator(table: &GridTable, layout: &Layout, boundary: usize) -> String {
    let row_count = layout.sections.len();
    let section_above = boundary.checked_sub(1).and_then(|r| layout.sections.get(r));
    let section_below = layout.sections.get(boundary);
    let fill = match (section_above, section_below) {
        (None, _) => '-',
        (Some(above), Some(below)) if above == below => '-',
        (Some(_), Some(_)) => '=',
        (Some(_), None) => {
            if table.foot.is_empty() {
                '-'
            } else {
                '='
            }
        }
    };
    let marks_alignment = match (section_above, section_below) {
        // The boundary that closes the head carries the marks; with no head, the top border.
        (Some(Section::Head), Some(below)) => below != &Section::Head,
        (None, _) => table.head.is_empty(),
        _ => false,
    };

    let continuing: Vec<bool> = (0..table.col_widths.len())
        .map(|col| {
            boundary > 0
                && boundary < row_count
                && layout
                    .occupancy
                    .get(boundary - 1)
                    .and_then(|slots| slots.get(col))
                    == layout
                        .occupancy
                        .get(boundary)
                        .and_then(|slots| slots.get(col))
        })
        .collect();

    let mut line = String::new();
    for (col, &width) in table.col_widths.iter().enumerate() {
        line.push(junction(&continuing, col, table.col_widths.len()));
        if continuing.get(col).copied().unwrap_or(false) {
            line.extend(std::iter::repeat_n(' ', width));
        } else {
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
        }
    }
    line.push(junction(
        &continuing,
        table.col_widths.len(),
        table.col_widths.len(),
    ));
    line
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

/// The character where a separator line crosses the boundary before column `col`: `+` next to
/// any drawn segment, `|` at the table edge along a spanning cell, blank inside one.
fn junction(continuing: &[bool], col: usize, columns: usize) -> char {
    let left_drawn = col > 0 && !continuing.get(col - 1).copied().unwrap_or(true);
    let right_drawn = col < columns && !continuing.get(col).copied().unwrap_or(true);
    if left_drawn || right_drawn {
        '+'
    } else if col == 0 || col == columns {
        '|'
    } else {
        ' '
    }
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
        // A spanning cell's content renders entirely within its first row's lines; the rows it
        // covers below show blanks.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(text: &str) -> GridCell {
        GridCell {
            lines: if text.is_empty() {
                Vec::new()
            } else {
                text.split('\n').map(str::to_owned).collect()
            },
            row_span: 1,
            col_span: 1,
        }
    }

    fn spanning(text: &str, row_span: usize, col_span: usize) -> GridCell {
        GridCell {
            lines: text.split('\n').map(str::to_owned).collect(),
            row_span,
            col_span,
        }
    }

    fn row(cells: Vec<GridCell>) -> GridRow {
        GridRow { cells }
    }

    fn render_table(
        col_widths: Vec<usize>,
        head: Vec<GridRow>,
        body: Vec<GridRow>,
        foot: Vec<GridRow>,
    ) -> String {
        render(&GridTable {
            col_widths,
            aligns: None,
            head,
            body,
            foot,
        })
    }

    #[test]
    fn headed_table_draws_an_equals_boundary() {
        let output = render_table(
            vec![4, 4],
            vec![row(vec![cell("h1"), cell("h2")])],
            vec![row(vec![cell("a"), cell("b")])],
            Vec::new(),
        );
        assert_eq!(
            output,
            "+----+----+\n\
             | h1 | h2 |\n\
             +====+====+\n\
             | a  | b  |\n\
             +----+----+"
        );
    }

    #[test]
    fn row_span_blanks_the_separator_and_rows_below() {
        let output = render_table(
            vec![6, 3],
            Vec::new(),
            vec![
                row(vec![spanning("tall", 2, 1), cell("a")]),
                row(vec![cell("b")]),
            ],
            Vec::new(),
        );
        assert_eq!(
            output,
            "+------+---+\n\
             | tall | a |\n\
             |      +---+\n\
             |      | b |\n\
             +------+---+"
        );
    }

    #[test]
    fn interior_row_span_gets_plus_junctions_on_both_sides() {
        let output = render_table(
            vec![3, 6, 3],
            Vec::new(),
            vec![
                row(vec![cell("a"), spanning("tall", 2, 1), cell("b")]),
                row(vec![cell("c"), cell("d")]),
            ],
            Vec::new(),
        );
        assert_eq!(
            output,
            "+---+------+---+\n\
             | a | tall | b |\n\
             +---+      +---+\n\
             | c |      | d |\n\
             +---+------+---+"
        );
    }

    #[test]
    fn row_span_at_the_right_edge_keeps_the_edge_bar() {
        let output = render_table(
            vec![3, 6],
            Vec::new(),
            vec![
                row(vec![cell("a"), spanning("tall", 2, 1)]),
                row(vec![cell("b")]),
            ],
            Vec::new(),
        );
        assert_eq!(
            output,
            "+---+------+\n\
             | a | tall |\n\
             +---+      |\n\
             | b |      |\n\
             +---+------+"
        );
    }

    #[test]
    fn column_span_merges_the_field_but_not_the_borders() {
        let output = render_table(
            vec![4, 4],
            Vec::new(),
            vec![
                row(vec![spanning("wide", 1, 2)]),
                row(vec![cell("a"), cell("b")]),
            ],
            Vec::new(),
        );
        assert_eq!(
            output,
            "+----+----+\n\
             | wide    |\n\
             +----+----+\n\
             | a  | b  |\n\
             +----+----+"
        );
    }

    #[test]
    fn rectangular_span_blanks_interior_junctions() {
        let output = render_table(
            vec![4, 4, 3],
            Vec::new(),
            vec![
                row(vec![spanning("span", 2, 2), cell("a")]),
                row(vec![cell("b")]),
            ],
            Vec::new(),
        );
        assert_eq!(
            output,
            "+----+----+---+\n\
             | span    | a |\n\
             |         +---+\n\
             |         | b |\n\
             +----+----+---+"
        );
    }

    #[test]
    fn spanning_cell_content_renders_in_its_first_row_region() {
        let output = render_table(
            vec![7, 3],
            Vec::new(),
            vec![
                row(vec![spanning("one\n\ntwo\n\nthree", 2, 1), cell("a")]),
                row(vec![cell("b")]),
            ],
            Vec::new(),
        );
        assert_eq!(
            output,
            "+-------+---+\n\
             | one   | a |\n\
             |       |   |\n\
             | two   |   |\n\
             |       |   |\n\
             | three |   |\n\
             |       +---+\n\
             |       | b |\n\
             +-------+---+"
        );
    }

    #[test]
    fn foot_is_fenced_by_equals_separators() {
        let output = render_table(
            vec![5, 4],
            vec![row(vec![cell("h1"), cell("h2")])],
            vec![row(vec![cell("a"), cell("b")])],
            vec![row(vec![cell("sum"), cell("3")])],
        );
        assert_eq!(
            output,
            "+-----+----+\n\
             | h1  | h2 |\n\
             +=====+====+\n\
             | a   | b  |\n\
             +=====+====+\n\
             | sum | 3  |\n\
             +=====+====+"
        );
    }

    #[test]
    fn foot_rows_separate_with_dashes() {
        let output = render_table(
            vec![4, 4],
            vec![row(vec![cell("h1"), cell("h2")])],
            vec![row(vec![cell("a"), cell("b")])],
            vec![
                row(vec![cell("f1"), cell("f2")]),
                row(vec![cell("g1"), cell("g2")]),
            ],
        );
        assert_eq!(
            output,
            "+----+----+\n\
             | h1 | h2 |\n\
             +====+====+\n\
             | a  | b  |\n\
             +====+====+\n\
             | f1 | f2 |\n\
             +----+----+\n\
             | g1 | g2 |\n\
             +====+====+"
        );
    }

    #[test]
    fn head_meets_foot_directly_when_the_body_is_empty() {
        let output = render_table(
            vec![4, 4],
            vec![row(vec![cell("h1"), cell("h2")])],
            Vec::new(),
            vec![row(vec![cell("f1"), cell("f2")])],
        );
        assert_eq!(
            output,
            "+----+----+\n\
             | h1 | h2 |\n\
             +====+====+\n\
             | f1 | f2 |\n\
             +====+====+"
        );
    }

    #[test]
    fn multiple_head_rows_separate_with_dashes() {
        let output = render_table(
            vec![4, 4],
            vec![
                row(vec![cell("h1"), cell("h2")]),
                row(vec![cell("i1"), cell("i2")]),
            ],
            vec![row(vec![cell("a"), cell("b")])],
            Vec::new(),
        );
        assert_eq!(
            output,
            "+----+----+\n\
             | h1 | h2 |\n\
             +----+----+\n\
             | i1 | i2 |\n\
             +====+====+\n\
             | a  | b  |\n\
             +----+----+"
        );
    }

    #[test]
    fn alignment_colons_mark_the_head_boundary() {
        let output = render(&GridTable {
            col_widths: vec![7, 5, 4],
            aligns: Some(&[
                Alignment::AlignLeft,
                Alignment::AlignRight,
                Alignment::AlignCenter,
            ]),
            head: vec![row(vec![cell("l"), cell("r"), cell("c")])],
            body: vec![row(vec![cell("1"), cell("2"), cell("3")])],
            foot: Vec::new(),
        });
        assert_eq!(
            output,
            "+-------+-----+----+\n\
             | l     | r   | c  |\n\
             +:======+====:+:==:+\n\
             | 1     | 2   | 3  |\n\
             +-------+-----+----+"
        );
    }

    #[test]
    fn headerless_alignment_colons_move_to_the_top_border() {
        let output = render(&GridTable {
            col_widths: vec![6, 3],
            aligns: Some(&[Alignment::AlignRight, Alignment::AlignCenter]),
            head: Vec::new(),
            body: vec![
                row(vec![spanning("tall", 2, 1), cell("a")]),
                row(vec![cell("b")]),
            ],
            foot: Vec::new(),
        });
        assert_eq!(
            output,
            "+-----:+:-:+\n\
             | tall | a |\n\
             |      +---+\n\
             |      | b |\n\
             +------+---+"
        );
    }

    #[test]
    fn aligned_foot_boundaries_stay_unmarked() {
        let output = render(&GridTable {
            col_widths: vec![4, 4],
            aligns: Some(&[Alignment::AlignLeft, Alignment::AlignRight]),
            head: vec![row(vec![cell("h1"), cell("h2")])],
            body: vec![row(vec![cell("a"), cell("b")])],
            foot: vec![row(vec![cell("f1"), cell("f2")])],
        });
        assert_eq!(
            output,
            "+----+----+\n\
             | h1 | h2 |\n\
             +:===+===:+\n\
             | a  | b  |\n\
             +====+====+\n\
             | f1 | f2 |\n\
             +====+====+"
        );
    }

    #[test]
    fn short_rows_fill_with_blank_cells() {
        let output = render_table(
            vec![6, 3, 3],
            Vec::new(),
            vec![
                row(vec![spanning("tall", 2, 1), cell("a"), cell("b")]),
                row(vec![cell("c")]),
            ],
            Vec::new(),
        );
        assert_eq!(
            output,
            "+------+---+---+\n\
             | tall | a | b |\n\
             |      +---+---+\n\
             |      | c |   |\n\
             +------+---+---+"
        );
    }

    #[test]
    fn span_clamps_at_its_section_boundary() {
        let output = render_table(
            vec![6, 4],
            Vec::new(),
            vec![
                row(vec![spanning("tall", 5, 1), cell("a")]),
                row(vec![cell("b")]),
            ],
            vec![row(vec![cell("f1"), cell("f2")])],
        );
        assert_eq!(
            output,
            "+------+----+\n\
             | tall | a  |\n\
             |      +----+\n\
             |      | b  |\n\
             +======+====+\n\
             | f1   | f2 |\n\
             +======+====+"
        );
    }

    #[test]
    fn wide_characters_pad_by_display_width() {
        let output = render_table(
            vec![6, 14],
            vec![row(vec![cell("項目"), cell("値")])],
            vec![row(vec![cell("名前"), cell("漢字テキスト")])],
            Vec::new(),
        );
        assert_eq!(
            output,
            "+------+--------------+\n\
             | 項目 | 値           |\n\
             +======+==============+\n\
             | 名前 | 漢字テキスト |\n\
             +------+--------------+"
        );
    }

    #[test]
    fn empty_table_renders_nothing() {
        assert_eq!(
            render_table(Vec::new(), Vec::new(), Vec::new(), Vec::new()),
            ""
        );
        assert_eq!(
            render_table(vec![3], Vec::new(), Vec::new(), Vec::new()),
            ""
        );
    }

    #[test]
    fn every_line_has_the_same_display_width() {
        let output = render_table(
            vec![6, 3, 5],
            vec![row(vec![cell("h"), spanning("wide", 1, 2)])],
            vec![
                row(vec![spanning("tall", 2, 2), cell("x")]),
                row(vec![cell("y")]),
            ],
            Vec::new(),
        );
        let widths: Vec<usize> = output.lines().map(display_width).collect();
        assert!(widths.iter().all(|&w| w == 6 + 3 + 5 + 4), "{output}");
    }
}
