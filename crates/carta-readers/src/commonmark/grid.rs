//! Grid tables: a block ruled by `+`/`|` whose columns are fixed by the `+` positions on its border
//! lines. The top border `+---+---+` sets the column boundaries; a `+===+` divider, when present,
//! splits the rows above it into a header. Each cell holds its raw text, sliced out by column and
//! left for the inline phase to parse as block content; column widths and alignments come from the
//! border lines.

use carta_ast::{Alignment, Attr};

/// A parsed grid table awaiting the inline phase: one [`Column`] per column plus the header, body,
/// and footer rows of raw cell text. The caption text, stripped of its marker, is attached after
/// parsing.
#[derive(Debug, Clone)]
pub(crate) struct GridTable {
    pub columns: Vec<Column>,
    pub head: Vec<Row>,
    pub body: Vec<Row>,
    pub foot: Vec<Row>,
    pub caption: Option<String>,
    /// Attributes attached via the caption line when `table_attributes` is enabled.
    pub attr: Attr,
}

/// One column's alignment and its width as a fraction of the text width.
#[derive(Debug, Clone)]
pub(crate) struct Column {
    pub align: Alignment,
    pub width: f64,
}

/// One table row: a cell per column.
#[derive(Debug, Clone)]
pub(crate) struct Row {
    pub cells: Vec<Cell>,
}

/// One cell's raw text and whether it is tight (no internal blank line, so its paragraph renders as
/// `Plain` rather than `Para`).
#[derive(Debug, Clone)]
pub(crate) struct Cell {
    pub text: String,
    pub tight: bool,
}

/// The text-width denominator: a column's width is its border span divided by this.
const TEXT_WIDTH: f64 = 72.0;

/// Whether `line` is a valid grid top border: a `+...+` ruling of `+`, `-`, and alignment colons,
/// with at least one column. A `=` is rejected; it belongs only to the header divider, never the
/// top border.
pub(crate) fn is_top_border(line: &str) -> bool {
    is_ruling(trim(line), false)
}

/// Whether `line` belongs to a grid table once one is under way: a content line led by `|`, or any
/// ruling border line (the top/row borders or a `=` header divider).
pub(crate) fn is_grid_line(line: &str) -> bool {
    let trimmed = trim(line);
    trimmed.starts_with('|') || is_ruling(trimmed, true)
}

/// Strip leading block indentation and trailing whitespace.
fn trim(line: &str) -> &str {
    line.trim_start_matches(' ').trim_end()
}

/// Whether `s` (already trimmed) is a ruling line: `+`-delimited segments of `-`, `:`, and (when
/// `allow_eq`) `=`, with no empty segment.
fn is_ruling(s: &str, allow_eq: bool) -> bool {
    let bytes = s.as_bytes();
    bytes.len() >= 3
        && bytes.first() == Some(&b'+')
        && bytes.last() == Some(&b'+')
        && !s.contains("++")
        && s.bytes()
            .all(|c| matches!(c, b'+' | b'-' | b':') || (allow_eq && c == b'='))
}

/// Parse the accumulated text of a grid-table candidate. Returns `None` when the text is not a
/// complete, well-formed grid table (no top border, no closing border, a stray non-grid line, or no
/// content row), so the caller keeps it as an ordinary paragraph. Row- and column-spanning tables,
/// whose borders break the regular grid, are also returned as `None`: this parser models only the
/// rectangular grid where every border's `+` markers share one set of column boundaries.
pub(crate) fn parse(text: &str) -> Option<GridTable> {
    let lines: Vec<&str> = text.lines().map(trim).collect();
    let top = *lines.first()?;
    if !is_ruling(top, false) {
        return None;
    }
    let boundaries = border_columns(top);
    let column_count = boundaries.len().checked_sub(1)?;
    if column_count == 0 {
        return None;
    }
    // The candidate must close with a border; a trailing content line means it is incomplete.
    if !lines.last().is_some_and(|line| is_ruling(line, true)) {
        return None;
    }

    let mut rows: Vec<Row> = Vec::new();
    let mut pending: Vec<&str> = Vec::new();
    // Row counts before each `=` divider: the section breaks splitting header, body, and footer.
    let mut dividers: Vec<usize> = Vec::new();

    for (idx, line) in lines.iter().enumerate() {
        if is_ruling(line, true) {
            // Pluses off the shared boundaries, or alignment colons anywhere but the top border and
            // `=` divider, imply a span; spans are not modeled, so leave the candidate as prose.
            if border_columns(line) != boundaries {
                return None;
            }
            if idx > 0 && !line.contains('=') && line.contains(':') {
                return None;
            }
            // The top border opens the table; each later border closes the preceding segment into a row, even empty.
            if idx > 0 {
                rows.push(build_row(&pending, &boundaries));
                pending.clear();
            }
            if line.contains('=') {
                dividers.push(rows.len());
            }
        } else if line.starts_with('|') {
            // A content line needs a `|` at every boundary; a gap would merge cells (an unmodeled span).
            if !has_column_separators(line, &boundaries) {
                return None;
            }
            pending.push(line);
        } else {
            return None;
        }
    }
    if rows.is_empty() {
        return None;
    }

    // Alignment comes from the `=` divider when present, else the top border; row separators never set it.
    let mut aligns = vec![Alignment::AlignDefault; column_count];
    let align_source = lines
        .iter()
        .copied()
        .find(|line| is_ruling(line, true) && line.contains('='))
        .unwrap_or(top);
    collect_aligns(align_source, &boundaries, &mut aligns);

    let columns = (0..column_count)
        .map(|i| Column {
            align: aligns.get(i).cloned().unwrap_or(Alignment::AlignDefault),
            width: column_width(&boundaries, i),
        })
        .collect();
    let (head, body, foot) = split_sections(rows, &dividers);
    Some(GridTable {
        columns,
        head,
        body,
        foot,
        caption: None,
        attr: Attr::default(),
    })
}

/// Partition the rows into header, body, and footer using the `=` divider positions. The rows before
/// the first divider are the header. A footer is present only when the closing border is itself a `=`
/// divider and at least one more `=` divider precedes it: then the rows after that last interior
/// divider are the footer. Otherwise every row past the header is body.
fn split_sections(mut rows: Vec<Row>, dividers: &[usize]) -> (Vec<Row>, Vec<Row>, Vec<Row>) {
    let total = rows.len();
    let Some(&first) = dividers.first() else {
        return (Vec::new(), rows, Vec::new());
    };
    let closes_with_divider = dividers.last() == Some(&total);
    if closes_with_divider && dividers.len() >= 2 {
        let foot_start = dividers.get(dividers.len() - 2).copied().unwrap_or(total);
        let foot = rows.split_off(foot_start.min(rows.len()));
        let body = rows.split_off(first.min(rows.len()));
        (rows, body, foot)
    } else {
        let body = rows.split_off(first.min(rows.len()));
        (rows, body, Vec::new())
    }
}

/// Whether `line` has a column separator (`|` or `+`) at every column boundary it reaches. A
/// boundary that falls inside the line but holds another character marks a merged column.
fn has_column_separators(line: &str, boundaries: &[usize]) -> bool {
    let chars: Vec<char> = line.chars().collect();
    boundaries.iter().all(|&b| match chars.get(b) {
        Some(&c) => c == '|' || c == '+',
        None => false,
    })
}

/// The character positions of the `+` markers on a border line: the column boundaries.
fn border_columns(border: &str) -> Vec<usize> {
    border
        .chars()
        .enumerate()
        .filter(|&(_, c)| c == '+')
        .map(|(i, _)| i)
        .collect()
}

/// Column `i`'s width: its border span (the `+`-to-`+` distance, markers included) over the text
/// width.
#[allow(clippy::cast_precision_loss)] // border spans are small line offsets, far inside f64's range
fn column_width(boundaries: &[usize], i: usize) -> f64 {
    match (boundaries.get(i), boundaries.get(i + 1)) {
        (Some(&start), Some(&end)) => (end.saturating_sub(start) as f64) / TEXT_WIDTH,
        _ => 0.0,
    }
}

/// Read each column's alignment from a border line. A leading `:` marks left, a trailing `:` marks
/// right, both mark center, neither leaves the default.
fn collect_aligns(border: &str, boundaries: &[usize], aligns: &mut [Alignment]) {
    let chars: Vec<char> = border.chars().collect();
    for (i, slot) in aligns.iter_mut().enumerate() {
        let segment = column_segment(&chars, boundaries, i);
        let left = segment.first() == Some(&':');
        let right = segment.last() == Some(&':');
        *slot = match (left, right) {
            (true, true) => Alignment::AlignCenter,
            (true, false) => Alignment::AlignLeft,
            (false, true) => Alignment::AlignRight,
            (false, false) => Alignment::AlignDefault,
        };
    }
}

/// The characters strictly between column `i`'s two `+` boundaries on a line of `chars`.
fn column_segment<'a>(chars: &'a [char], boundaries: &[usize], i: usize) -> &'a [char] {
    let Some(&start) = boundaries.get(i) else {
        return &[];
    };
    let Some(&end) = boundaries.get(i + 1) else {
        return &[];
    };
    let lo = (start + 1).min(chars.len());
    let hi = end.min(chars.len());
    chars.get(lo..hi).unwrap_or(&[])
}

/// Build one row by slicing each content line at the column boundaries and assembling each column's
/// cell text.
fn build_row(content: &[&str], boundaries: &[usize]) -> Row {
    let column_count = boundaries.len().saturating_sub(1);
    let line_chars: Vec<Vec<char>> = content.iter().map(|line| line.chars().collect()).collect();
    let cells = (0..column_count)
        .map(|i| {
            let column_lines: Vec<String> = line_chars
                .iter()
                .map(|chars| column_segment(chars, boundaries, i).iter().collect())
                .collect();
            cell_text(column_lines)
        })
        .collect();
    Row { cells }
}

/// Assemble a cell from its per-line slices: drop the single padding space after the column edge,
/// drop trailing whitespace, and trim surrounding blank lines. Indentation beyond that one space is
/// kept, so a deeply indented cell line parses as an indented code block. A cell that keeps an
/// internal blank line is loose.
fn cell_text(lines: Vec<String>) -> Cell {
    let mut lines: Vec<String> = lines
        .into_iter()
        .map(|line| {
            line.strip_prefix(' ')
                .unwrap_or(&line)
                .trim_end()
                .to_owned()
        })
        .collect();
    while lines.first().is_some_and(String::is_empty) {
        lines.remove(0);
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    let text = lines.join("\n");
    let tight = !text.contains("\n\n");
    Cell { text, tight }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_at(rows: &[Row], row: usize, col: usize) -> &str {
        rows.get(row)
            .and_then(|r| r.cells.get(col))
            .map(|c| c.text.as_str())
            .expect("cell present")
    }

    fn tight_at(rows: &[Row], row: usize, col: usize) -> bool {
        rows.get(row)
            .and_then(|r| r.cells.get(col))
            .map(|c| c.tight)
            .expect("cell present")
    }

    fn col_align(table: &GridTable, col: usize) -> Option<&Alignment> {
        table.columns.get(col).map(|c| &c.align)
    }

    fn col_width(table: &GridTable, col: usize) -> f64 {
        table
            .columns
            .get(col)
            .map(|c| c.width)
            .expect("column present")
    }

    #[test]
    fn top_border_accepts_dashes_and_colons_only() {
        assert!(is_top_border("+---+---+"));
        assert!(is_top_border("+:--+--:+"));
        assert!(is_top_border("+-+"));
        assert!(is_top_border("   +---+"));
        // A `=` belongs to the header divider, not the top border.
        assert!(!is_top_border("+===+"));
        // A bullet marker, an empty segment, and bare runs are not borders.
        assert!(!is_top_border("+ item"));
        assert!(!is_top_border("++"));
        assert!(!is_top_border("+++"));
        assert!(!is_top_border("| a |"));
    }

    #[test]
    fn grid_line_accepts_content_and_any_border() {
        assert!(is_grid_line("| a | b |"));
        assert!(is_grid_line("+===+===+"));
        assert!(is_grid_line("+---+---+"));
        assert!(!is_grid_line("+ bullet"));
        assert!(!is_grid_line("plain text"));
    }

    #[test]
    fn parse_rejects_incomplete_tables() {
        // No closing border.
        assert!(parse("+---+\n| a |").is_none());
        // A lone border has no content row.
        assert!(parse("+---+").is_none());
        // A stray non-grid line breaks the structure.
        assert!(parse("+---+\nplain\n+---+").is_none());
    }

    #[test]
    fn parse_splits_header_at_the_equals_divider() {
        let table = parse("+---+---+\n| a | b |\n+===+===+\n| c | d |\n+---+---+").unwrap();
        assert_eq!(table.head.len(), 1);
        assert_eq!(table.body.len(), 1);
        assert_eq!(table.columns.len(), 2);
        assert_eq!(text_at(&table.head, 0, 0), "a");
        assert_eq!(text_at(&table.body, 0, 1), "d");
    }

    #[test]
    fn parse_without_divider_is_all_body() {
        let table = parse("+---+\n| a |\n+---+\n| b |\n+---+").unwrap();
        assert!(table.head.is_empty());
        assert_eq!(table.body.len(), 2);
    }

    #[test]
    fn alignment_comes_from_the_top_border_without_a_divider() {
        let table = parse("+:--+--:+\n| a | b |\n+---+---+").unwrap();
        assert!(matches!(col_align(&table, 0), Some(Alignment::AlignLeft)));
        assert!(matches!(col_align(&table, 1), Some(Alignment::AlignRight)));
        let centered = parse("+:-:+\n| a |\n+---+").unwrap();
        assert!(matches!(
            col_align(&centered, 0),
            Some(Alignment::AlignCenter)
        ));
    }

    #[test]
    fn the_header_divider_overrides_the_top_border_for_alignment() {
        // The `=` divider's colons win; the top border's are ignored once a header is present.
        let table = parse("+:--+:--+\n| a | b |\n+==:+==:+\n| c | d |\n+---+---+").unwrap();
        assert!(matches!(col_align(&table, 0), Some(Alignment::AlignRight)));
        assert!(matches!(col_align(&table, 1), Some(Alignment::AlignRight)));
    }

    #[test]
    fn a_colon_bearing_closing_border_is_not_a_table() {
        // Alignment colons are valid on the top border and the header divider, never the close.
        assert!(parse("+---+\n| a |\n+:-:+").is_none());
        assert!(parse("+---+\n| a |\n+--:+").is_none());
    }

    #[test]
    fn deep_cell_indentation_survives_as_code() {
        // Only the one padding space is stripped, so five leading spaces stay an indented block.
        let table = parse("+----------+\n|     code |\n+----------+").unwrap();
        assert_eq!(text_at(&table.body, 0, 0), "    code");
    }

    #[test]
    fn width_is_the_border_span_over_text_width() {
        let table = parse("+---+\n| a |\n+---+").unwrap();
        assert!((col_width(&table, 0) - 4.0 / 72.0).abs() < 1e-12);
    }

    #[test]
    fn multi_line_cell_is_tight_until_a_blank_line() {
        let tight = parse("+------+\n| one  |\n| two  |\n+------+").unwrap();
        assert!(tight_at(&tight.body, 0, 0));
        assert_eq!(text_at(&tight.body, 0, 0), "one\ntwo");
        let loose = parse("+------+\n| one  |\n|      |\n| two  |\n+------+").unwrap();
        assert!(!tight_at(&loose.body, 0, 0));
        assert_eq!(text_at(&loose.body, 0, 0), "one\n\ntwo");
    }

    #[test]
    fn empty_cell_has_empty_text() {
        let table = parse("+---+---+\n| a |   |\n+---+---+").unwrap();
        assert_eq!(text_at(&table.body, 0, 1), "");
    }

    #[test]
    fn a_second_divider_before_an_equals_close_makes_a_footer() {
        let table = parse("+---+\n| h |\n+===+\n| b |\n+===+\n| f |\n+===+").unwrap();
        assert_eq!(table.head.len(), 1);
        assert_eq!(table.body.len(), 1);
        assert_eq!(table.foot.len(), 1);
        assert_eq!(text_at(&table.head, 0, 0), "h");
        assert_eq!(text_at(&table.body, 0, 0), "b");
        assert_eq!(text_at(&table.foot, 0, 0), "f");
        // A plain `-` closing border keeps the trailing rows in the body, with no footer.
        let no_foot = parse("+---+\n| h |\n+===+\n| b |\n+===+\n| f |\n+---+").unwrap();
        assert!(no_foot.foot.is_empty());
        assert_eq!(no_foot.body.len(), 2);
    }

    #[test]
    fn spanning_candidates_are_declined() {
        // A content line missing a `|` at a boundary would merge columns (a column span).
        assert!(parse("+---+---+\n| ab    |\n+---+---+").is_none());
        // A border whose pluses fall off the column boundaries implies a span.
        assert!(parse("+---+---+\n| a | b |\n+-------+\n| cd    |\n+---+---+").is_none());
    }

    #[test]
    fn adjacent_borders_make_an_empty_row() {
        // Two borders with no content line between them frame an empty row.
        let table = parse("+---+\n| a |\n+---+\n+---+\n| b |\n+---+").unwrap();
        assert_eq!(table.body.len(), 3);
        assert_eq!(text_at(&table.body, 0, 0), "a");
        assert_eq!(text_at(&table.body, 1, 0), "");
        assert_eq!(text_at(&table.body, 2, 0), "b");
    }
}
