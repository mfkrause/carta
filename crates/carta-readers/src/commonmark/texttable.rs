//! Dash-ruled tables: blocks whose columns are fixed by runs of `-` rather than the `|`/`+` borders
//! of a pipe or grid table. Two shapes share this module:
//!
//! * a *headed* table — one header line above a dash ruling, then one row per line; and
//! * a *headerless* table — a dash ruling, then one row per line, closed by a second ruling.
//!
//! Both forms slice each line into cells at the dash runs' start columns, read per-column alignment
//! from a reference line (the header, or the first row when headerless), and leave each cell's text
//! raw for the inline phase. Column widths are unset here; the multi-line variant that derives
//! fractional widths from the ruling is layered on separately.

use carta_ast::Alignment;
use carta_core::{Extension, Extensions};

/// A parsed dash-ruled table awaiting the inline phase: one [`Column`] per column, an optional
/// header row, the body rows, and any caption text (stripped of its marker, attached after the
/// block phase).
#[derive(Debug, Clone)]
pub(crate) struct TextTable {
    pub columns: Vec<Column>,
    /// Header cells, one per column; empty when the table has no header row.
    pub head: Vec<Cell>,
    /// Body rows, each a cell per column.
    pub body: Vec<Vec<Cell>>,
    pub caption: Option<String>,
}

/// One column's alignment and width. `width` is `None` for an unsized column (the only kind a
/// single-line table produces).
#[derive(Debug, Clone)]
pub(crate) struct Column {
    pub align: Alignment,
    pub width: Option<f64>,
}

/// A cell's raw text: physical lines joined with `\n`, each break becoming a soft break when the
/// inline phase parses the text.
pub(crate) type Cell = String;

/// Whether `line` is a dash ruling: after trimming, a non-empty run of `-` and spaces with at least
/// one dash.
pub(crate) fn is_dash_line(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty()
        && trimmed.bytes().all(|byte| matches!(byte, b'-' | b' '))
        && trimmed.contains('-')
}

/// Whether `line` opens a dash-led table: a dash ruling (only dashes and spaces) whose first run is
/// at least two dashes. Requiring a pure ruling keeps grid-table borders (`+---+`) and other
/// dash-bearing lines from opening one; a lone leading dash is a list marker, so it does not either.
pub(crate) fn opens_dash_table(line: &str) -> bool {
    is_dash_line(line)
        && dash_runs(line)
            .first()
            .is_some_and(|&(_, len)| len >= 2)
}

/// The maximal runs of `-` in `line`, each as `(start, length)` in character positions.
fn dash_runs(line: &str) -> Vec<(usize, usize)> {
    let mut runs = Vec::new();
    let mut run_start: Option<usize> = None;
    let mut count = 0;
    for (index, ch) in line.chars().enumerate() {
        count = index + 1;
        if ch == '-' {
            run_start.get_or_insert(index);
        } else if let Some(start) = run_start.take() {
            runs.push((start, index - start));
        }
    }
    if let Some(start) = run_start {
        runs.push((start, count - start));
    }
    runs
}

/// Parse the accumulated lines of a dash-table candidate, returning the table and how many of the
/// lines it consumed (any trailing lines belong to the next block). Returns `None` when the lines do
/// not form a complete, well-formed table, so the caller can fall back. A leading dash ruling takes
/// the headerless shapes; any other first line takes the headed shape, with the ruling on the
/// second line.
pub(crate) fn parse(lines: &[&str], ext: Extensions) -> Option<(TextTable, usize)> {
    let first = *lines.first()?;
    if is_dash_line(first) {
        parse_headerless(lines, ext)
    } else {
        parse_headed(lines, ext)
    }
}

/// Whether either dash-table extension is enabled.
fn enabled(ext: Extensions) -> bool {
    ext.contains(Extension::SimpleTables) || ext.contains(Extension::MultilineTables)
}

/// Parse a headed table: a header line, a dash ruling, then one single-line row each until a closing
/// ruling (consumed) or the lines run out. Alignment comes from the header line; widths are unset.
fn parse_headed(lines: &[&str], ext: Extensions) -> Option<(TextTable, usize)> {
    if !enabled(ext) {
        return None;
    }
    let header = *lines.first()?;
    let ruling = *lines.get(1)?;
    if !is_dash_line(ruling) {
        return None;
    }
    let runs = dash_runs(ruling);
    if runs.is_empty() {
        return None;
    }
    let starts: Vec<usize> = runs.iter().map(|&(start, _)| start).collect();

    let mut body = Vec::new();
    let mut consumed = 2;
    for &line in lines.get(2..).unwrap_or(&[]) {
        consumed += 1;
        if is_dash_line(line) {
            break;
        }
        if line.trim().is_empty() {
            return None;
        }
        body.push(slice_row(line, &starts));
    }
    if body.is_empty() {
        return None;
    }

    let columns = column_specs(&runs, header);
    Some((
        TextTable {
            columns,
            head: slice_row(header, &starts),
            body,
            caption: None,
        },
        consumed,
    ))
}

/// Parse a headerless table: a dash ruling, then one single-line row each, closed by a second ruling
/// (required). Alignment comes from the first row; widths are unset.
fn parse_headerless(lines: &[&str], ext: Extensions) -> Option<(TextTable, usize)> {
    if !enabled(ext) {
        return None;
    }
    let top = *lines.first()?;
    if !is_dash_line(top) {
        return None;
    }
    let runs = dash_runs(top);
    if runs.is_empty() {
        return None;
    }
    let starts: Vec<usize> = runs.iter().map(|&(start, _)| start).collect();

    let mut body = Vec::new();
    let mut consumed = 1;
    let mut closed = false;
    for &line in lines.get(1..).unwrap_or(&[]) {
        consumed += 1;
        if is_dash_line(line) {
            closed = true;
            break;
        }
        if line.trim().is_empty() {
            return None;
        }
        body.push(slice_row(line, &starts));
    }
    if !closed || body.is_empty() {
        return None;
    }

    let reference = lines.get(1).copied().unwrap_or("");
    let columns = column_specs(&runs, reference);
    Some((
        TextTable {
            columns,
            head: Vec::new(),
            body,
            caption: None,
        },
        consumed,
    ))
}

/// Build the column specs from the dash runs and a reference line that fixes each column's
/// alignment. Single-line tables leave every width unset.
fn column_specs(runs: &[(usize, usize)], reference: &str) -> Vec<Column> {
    let chars: Vec<char> = reference.chars().collect();
    runs.iter()
        .enumerate()
        .map(|(index, &(start, len))| {
            let next = runs
                .get(index + 1)
                .map_or(usize::MAX, |&(next_start, _)| next_start);
            Column {
                align: column_alignment(&chars, start, len, next),
                width: None,
            }
        })
        .collect()
}

/// A column's alignment, read by comparing where the reference text sits to where the dashes are.
/// The column spans `[start, next)` and the dashes run `[start, start + len)`. Within the column,
/// the reference text's first and last non-blank columns fix its extent. Text indented from the
/// dashes' left edge frees the left side; text ending before the dashes' right edge frees the right.
/// Both free is centered, only the left free is right-aligned, only the right free is left-aligned,
/// neither is the default. A column whose reference text is blank keeps the default.
fn column_alignment(reference: &[char], start: usize, len: usize, next: usize) -> Alignment {
    let dash_end = start + len;
    let lo = start.min(reference.len());
    let hi = next.min(reference.len());
    let slice = reference.get(lo..hi).unwrap_or(&[]);
    let Some(first) = slice.iter().position(|ch| *ch != ' ') else {
        return Alignment::AlignDefault;
    };
    let last = slice.iter().rposition(|ch| *ch != ' ').unwrap_or(first);
    let text_start = lo + first;
    let text_end = lo + last + 1;
    let left_free = text_start > start;
    let right_free = text_end < dash_end;
    match (left_free, right_free) {
        (true, true) => Alignment::AlignCenter,
        (true, false) => Alignment::AlignRight,
        (false, true) => Alignment::AlignLeft,
        (false, false) => Alignment::AlignDefault,
    }
}

/// Slice one physical line into per-column text. Column `i` spans from run `i`'s start column to run
/// `i + 1`'s start column, with the last column running to the line's end; each piece is trimmed.
fn slice_row(line: &str, starts: &[usize]) -> Vec<Cell> {
    let chars: Vec<char> = line.chars().collect();
    starts
        .iter()
        .enumerate()
        .map(|(index, &start)| {
            let end = starts.get(index + 1).copied().unwrap_or(chars.len());
            let lo = start.min(chars.len());
            let hi = end.min(chars.len()).max(lo);
            chars
                .get(lo..hi)
                .map(|piece| piece.iter().collect::<String>())
                .unwrap_or_default()
                .trim()
                .to_owned()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE: Extensions = Extensions::from_list(&[Extension::SimpleTables]);

    fn cell(table: &TextTable, row: usize, col: usize) -> &str {
        table
            .body
            .get(row)
            .and_then(|cells| cells.get(col))
            .map(String::as_str)
            .expect("cell present")
    }

    fn head_cell(table: &TextTable, col: usize) -> &str {
        table.head.get(col).map(String::as_str).expect("header cell present")
    }

    fn align(table: &TextTable, col: usize) -> &Alignment {
        table.columns.get(col).map(|column| &column.align).expect("column present")
    }

    #[test]
    fn dash_line_accepts_only_dashes_and_spaces() {
        assert!(is_dash_line("-----"));
        assert!(is_dash_line("--- ---"));
        assert!(is_dash_line("  -- -- "));
        assert!(!is_dash_line(""));
        assert!(!is_dash_line("   "));
        assert!(!is_dash_line("--=--"));
        assert!(!is_dash_line("- x"));
    }

    #[test]
    fn opener_skips_list_markers_and_short_runs() {
        // A first run of two or more dashes opens a table candidate.
        assert!(opens_dash_table("--"));
        assert!(opens_dash_table("---- ----"));
        assert!(opens_dash_table("-- --"));
        // A single leading dash is a list marker, not a border.
        assert!(!opens_dash_table("-"));
        assert!(!opens_dash_table("- -"));
        assert!(!opens_dash_table("- - -"));
    }

    #[test]
    fn dash_runs_finds_positions_and_lengths() {
        assert_eq!(dash_runs("---- ---"), vec![(0, 4), (5, 3)]);
        assert_eq!(dash_runs("  -- --"), vec![(2, 2), (5, 2)]);
        assert_eq!(dash_runs(""), Vec::new());
    }

    #[test]
    fn headed_table_reads_header_and_rows() {
        let lines = ["Right Left Center", "----- ---- ------", "12    12   12"];
        let (table, consumed) = parse(&lines, SIMPLE).expect("table");
        assert_eq!(consumed, 3);
        assert_eq!(table.columns.len(), 3);
        assert_eq!(head_cell(&table, 0), "Right");
        assert_eq!(head_cell(&table, 2), "Center");
        assert_eq!(cell(&table, 0, 1), "12");
        // Header text flush within each run leaves the default alignment.
        assert!(matches!(align(&table, 0), Alignment::AlignDefault));
        assert!(table.head.iter().all(|_| true) && table.columns.iter().all(|c| c.width.is_none()));
    }

    #[test]
    fn headed_table_alignment_from_spacing() {
        // Run columns four wide: flush-left, centered, padded-right, and exactly filled.
        let lines = ["ab    cd  ef ghij", "---- ---- ---- ----", "X    Y    Z    W"];
        let (table, _) = parse(&lines, SIMPLE).expect("table");
        assert!(matches!(align(&table, 0), Alignment::AlignLeft));
        assert!(matches!(align(&table, 1), Alignment::AlignCenter));
        assert!(matches!(align(&table, 2), Alignment::AlignDefault));
        assert!(matches!(align(&table, 3), Alignment::AlignLeft));
    }

    #[test]
    fn headed_table_closes_at_trailing_ruling() {
        let lines = ["H1 H2", "-- --", "a  b", "-- --", "c  d"];
        let (table, consumed) = parse(&lines, SIMPLE).expect("table");
        // The second ruling closes the table; the row after it is left for the next block.
        assert_eq!(consumed, 4);
        assert_eq!(table.body.len(), 1);
        assert_eq!(cell(&table, 0, 0), "a");
    }

    #[test]
    fn headerless_table_needs_a_closing_ruling() {
        let closed = ["--- ---", "1 2", "---"];
        let (table, consumed) = parse(&closed, SIMPLE).expect("table");
        assert_eq!(consumed, 3);
        assert!(table.head.is_empty());
        assert_eq!(cell(&table, 0, 0), "1 2");
        assert_eq!(cell(&table, 0, 1), "");
        // Without the closing ruling there is no table.
        let open = ["--- ---", "1 2"];
        assert!(parse(&open, SIMPLE).is_none());
    }

    #[test]
    fn headerless_alignment_from_first_row() {
        let lines = ["-------- --------", "  r         l1", "x         y", "--------- -------"];
        let (table, _) = parse(&lines, SIMPLE).expect("table");
        // Column zero's first-row text is padded on both sides; column one's reaches the right edge.
        assert!(matches!(align(&table, 0), Alignment::AlignCenter));
    }

    #[test]
    fn gap_whitespace_joins_the_left_cell() {
        let lines = ["----- -----", "a         b", "-----------"];
        let (table, _) = parse(&lines, SIMPLE).expect("table");
        assert_eq!(cell(&table, 0, 0), "a");
        assert_eq!(cell(&table, 0, 1), "b");
    }

    #[test]
    fn disabled_extensions_parse_nothing() {
        let lines = ["--- ---", "1 2", "---"];
        assert!(parse(&lines, Extensions::empty()).is_none());
    }
}
