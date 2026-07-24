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
fn rectangular_span_merges_the_boundary_it_covers() {
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
        "+---------+---+\n\
             | span    | a |\n\
             |         +---+\n\
             |         | b |\n\
             +---------+---+"
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
fn header_only_table_closes_with_a_section_rule() {
    let output = render_table(
        vec![4, 4],
        vec![row(vec![cell("h1"), cell("h2")])],
        Vec::new(),
        Vec::new(),
    );
    assert_eq!(
        output,
        "+----+----+\n\
             | h1 | h2 |\n\
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

fn sized(fraction: f64) -> ColSpec {
    ColSpec {
        align: Alignment::AlignDefault,
        width: ColWidth::ColWidth(fraction),
    }
}

#[test]
fn colspan_width_floor_folds_the_remainder_back_into_each_column() {
    // Budget 35 over three columns: floor(35/3) = 11, plus the 35 % 3 = 2 leftover, minus one padding: 12.
    let three = [sized(13.0 / 72.0), sized(11.0 / 72.0), sized(11.0 / 72.0)];
    assert_eq!(colspan_width_floor(&three, 0, 3, 72), 12);

    // Even 22-character budget: floor(22/2) - 1 = 10.
    let two = [sized(11.0 / 72.0), sized(11.0 / 72.0)];
    assert_eq!(colspan_width_floor(&two, 0, 2, 72), 10);
}

#[test]
fn explicit_grid_widths_widen_spanned_columns_to_the_shared_floor() {
    let specs = [
        sized(25.0 / 72.0),
        sized(13.0 / 72.0),
        sized(11.0 / 72.0),
        sized(11.0 / 72.0),
    ];
    let natural = [1, 1, 1, 1];
    let minword = [1, 1, 1, 1];
    // Floors 12 (three-column span) and 10 (two-column span); the wider wins on the overlap.
    let widths = grid_content_widths(
        &specs,
        &natural,
        &minword,
        &[(1, 3), (2, 2)],
        4,
        72,
        WrapMode::Auto,
    );
    assert_eq!(widths, vec![22, 12, 12, 12]);
}

#[test]
fn explicit_grid_width_clamps_an_out_of_range_fraction_to_the_line() {
    // In-range fractions scale as floor(fraction * width) minus the border reservation.
    assert_eq!(explicit_grid_width(0.5, 72), 33);
    assert_eq!(explicit_grid_width(1.0, 72), 69);
    // Out-of-range fractions clamp to the line width first.
    assert_eq!(explicit_grid_width(1.0e53, 72), 69);
    assert_eq!(explicit_grid_width(f64::INFINITY, 72), 69);
}

#[test]
fn colspan_width_floor_clamps_an_out_of_range_fraction_to_the_line() {
    // An absurd fractional sum clamps to the line width, keeping covered columns bounded.
    let normal = [sized(11.0 / 72.0), sized(11.0 / 72.0)];
    assert_eq!(colspan_width_floor(&normal, 0, 2, 72), 10);
    let absurd = [sized(1.0e40), sized(1.0e40)];
    assert_eq!(colspan_width_floor(&absurd, 0, 2, 72), 35);
}

#[test]
fn out_of_range_fraction_keeps_grid_widths_bounded() {
    let specs = [sized(1.9e53), sized(0.0)];
    let natural = [1, 1];
    let minword = [1, 1];
    let widths = grid_content_widths(&specs, &natural, &minword, &[], 2, 72, WrapMode::Auto);
    assert!(
        widths.iter().all(|&w| w <= 72),
        "an absurd fraction must not inflate a column: {widths:?}"
    );
}
