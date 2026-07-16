//! Pipe-table recognition for the `pipe_tables` extension.
//!
//! A pipe table is a header row, a delimiter row that fixes the column count and per-column
//! alignment, and zero or more body rows. Recognition is purely textual and runs on a paragraph's
//! raw lines before inline parsing; the column-bearing cell text is parsed into inlines later. A
//! table never interrupts a paragraph, which falls out of only inspecting the first two lines of a
//! paragraph block.

use carta_ast::Alignment;

/// The header alignments, header cell texts, and body row cell texts of a recognized table.
pub(crate) type ParsedTable = (Vec<Alignment>, Vec<String>, Vec<Vec<String>>);

/// Recognize a pipe table at the start of `text`, returning its alignments, header cells, and body
/// rows, or `None` if the first two lines are not a header followed by a valid delimiter row.
///
/// A valid table requires the second line to parse as a delimiter row whose cell count equals the
/// header's. Body rows are every following line, padded with empty cells when short and truncated
/// when long.
///
/// When `code_spans` is set, a `|` inside a backtick code span is literal cell content, not a column
/// separator: the Markdown dialect keeps a code span whole, while pure `CommonMark` (and GFM) split
/// on every unescaped pipe.
pub(crate) fn try_parse(text: &str, code_spans: bool) -> Option<ParsedTable> {
    let mut lines = text.lines();
    let header_line = lines.next()?;
    let delimiter_line = lines.next()?;

    let (header, header_pipes) = split_cells(header_line, code_spans);
    if !is_pipe_row(&header, header_pipes) {
        return None;
    }
    let alignments = parse_delimiter(delimiter_line, code_spans)?;
    if header.len() != alignments.len() {
        return None;
    }
    let columns = alignments.len();

    let rows = lines
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let (mut cells, _) = split_cells(line, code_spans);
            cells.resize(columns, String::new());
            cells
        })
        .collect();

    Some((alignments, header, rows))
}

/// How a fresh line relates to an open paragraph that may be a pipe table.
pub(crate) enum Continuation {
    /// The line is a delimiter or body row; fold it into the paragraph as part of the table.
    Absorb,
    /// The paragraph is a complete table and the line is not a row; the table ends before it.
    Terminate,
    /// The paragraph is not a table; handle the line by the ordinary block rules.
    NotTable,
}

/// Decide how `line` extends an open paragraph holding `paragraph` text, so an established table can
/// keep claiming its rows before the block openers would split them.
///
/// Only a table whose header and delimiter rows are both already present takes over: a following
/// pipe row is a body row and anything else ends the table. While only the header is present the
/// candidate delimiter is left to the ordinary block rules, so a delimiter that begins with a list
/// marker (`- |`) opens a list rather than a table — a table must not interrupt those.
///
/// Once the header and delimiter have validated the caller passes `established`, and only the new
/// line is inspected: the header and delimiter rows never change, so re-verifying them for every
/// body line is wasted work.
pub(crate) fn classify_continuation(
    paragraph: &str,
    line: &str,
    code_spans: bool,
    established: bool,
) -> Continuation {
    if !established {
        let mut existing = paragraph.lines();
        let Some(header_line) = existing.next() else {
            return Continuation::NotTable;
        };
        let Some(delimiter_line) = existing.next() else {
            return Continuation::NotTable;
        };
        if !opens_table(header_line, delimiter_line, code_spans) {
            return Continuation::NotTable;
        }
    }
    let (cells, pipes) = split_cells(line, code_spans);
    if is_pipe_row(&cells, pipes) {
        Continuation::Absorb
    } else {
        Continuation::Terminate
    }
}

/// Whether `header_line` immediately followed by `delimiter_line` starts a pipe table: the header is
/// a pipe row and the delimiter parses to the same number of columns.
pub(crate) fn opens_table(header_line: &str, delimiter_line: &str, code_spans: bool) -> bool {
    let (header, header_pipes) = split_cells(header_line, code_spans);
    if !is_pipe_row(&header, header_pipes) {
        return false;
    }
    matches!(
        parse_delimiter(delimiter_line, code_spans),
        Some(alignments) if alignments.len() == header.len()
    )
}

/// Split one table row into its trimmed cell texts, alongside the count of unescaped `|` that act as
/// cell structure (every edge and separator pipe, excluding escaped and code-span pipes).
///
/// A single leading or trailing `|` is an edge delimiter and yields no cell; a doubled edge pipe
/// (or an interior `||`) yields an empty cell. A backslash-escaped `\|` is a literal pipe within a
/// cell, not a split point. When `code_spans` is set, a backtick code span is kept whole, so a `|`
/// inside it does not split the row.
///
/// The scan runs over the trimmed line's bytes: the structural characters (`|`, `\`, `` ` ``) are
/// ASCII, so cell boundaries land on byte offsets that are valid `&str` slice points even for
/// multibyte content. A cell that carries no escaped pipe is emitted as a single trimmed sub-slice;
/// only an escaped `\|` forces the cell to be assembled in a buffer with the backslash dropped.
fn split_cells(line: &str, code_spans: bool) -> (Vec<String>, usize) {
    let trimmed = line.trim();
    let bytes = trimmed.as_bytes();
    let leading_edge = bytes.first() == Some(&b'|');
    let mut i = usize::from(leading_edge);

    let mut cells = Vec::new();
    let mut pipes = usize::from(leading_edge);
    let mut run_start = i;
    let mut buffer = String::new();
    let mut ended_on_pipe = false;

    while let Some(&b) = bytes.get(i) {
        match b {
            b'`' if code_spans => {
                i = code_span_end(bytes, i).unwrap_or(i + 1);
                ended_on_pipe = false;
            }
            b'\\' if bytes.get(i + 1) == Some(&b'|') => {
                buffer.push_str(trimmed.get(run_start..i).unwrap_or(""));
                buffer.push('|');
                i += 2;
                run_start = i;
                ended_on_pipe = false;
            }
            b'|' => {
                push_cell(
                    &mut cells,
                    &mut buffer,
                    trimmed.get(run_start..i).unwrap_or(""),
                );
                pipes += 1;
                i += 1;
                run_start = i;
                ended_on_pipe = true;
            }
            _ => {
                i += 1;
                ended_on_pipe = false;
            }
        }
    }
    push_cell(
        &mut cells,
        &mut buffer,
        trimmed.get(run_start..).unwrap_or(""),
    );
    if ended_on_pipe {
        cells.pop();
    }

    (cells, pipes)
}

/// Finish a cell that ends with the literal run `tail`. When no escaped pipe buffered content for
/// this cell, the cell is exactly `tail` and is emitted as one trimmed slice; otherwise `tail` joins
/// the buffered prefix and the whole is trimmed. `buffer` is left empty, ready for the next cell.
fn push_cell(cells: &mut Vec<String>, buffer: &mut String, tail: &str) {
    if buffer.is_empty() {
        cells.push(tail.trim().to_owned());
    } else {
        buffer.push_str(tail);
        cells.push(buffer.trim().to_owned());
        buffer.clear();
    }
}

/// If a backtick code span opens at `bytes[start]`, return the offset just past its closing run. A
/// code span is a run of N backticks closed by the next run of exactly N backticks; a run of a
/// different length is ordinary content and the scan continues past it. An unclosed run is not a
/// span, so `None` leaves the opening backtick as ordinary text. Backticks are ASCII, so a run
/// boundary is always a valid `&str` slice point.
fn code_span_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut open = start;
    while bytes.get(open) == Some(&b'`') {
        open += 1;
    }
    let run = open - start;
    let mut i = open;
    while let Some(&b) = bytes.get(i) {
        if b == b'`' {
            let mut close = i;
            while bytes.get(close) == Some(&b'`') {
                close += 1;
            }
            if close - i == run {
                return Some(close);
            }
            i = close;
        } else {
            i += 1;
        }
    }
    None
}

/// Parse a delimiter row into its per-column alignments, or `None` if it is not a pipe row or any
/// cell is not a valid `:?-+:?` run.
fn parse_delimiter(line: &str, code_spans: bool) -> Option<Vec<Alignment>> {
    let (cells, pipes) = split_cells(line, code_spans);
    if !is_pipe_row(&cells, pipes) {
        return None;
    }
    cells.iter().map(|cell| delimiter_align(cell)).collect()
}

/// Whether a line carries enough pipe structure to be a table header or delimiter row, given its
/// split `cells` and the count of unescaped structural `pipes` from [`split_cells`]. A row needs two
/// or more pipes, or a single pipe alongside some non-empty cell; a lone `|` or a pipeless line is
/// not a row.
fn is_pipe_row(cells: &[String], pipes: usize) -> bool {
    pipes >= 2 || (pipes >= 1 && cells.iter().any(|cell| !cell.is_empty()))
}

/// Read a single delimiter cell's alignment: an optional leading colon, one or more dashes, an
/// optional trailing colon, and nothing else. The colons select the alignment.
fn delimiter_align(cell: &str) -> Option<Alignment> {
    let mut chars = cell.chars().peekable();
    let left = chars.peek() == Some(&':');
    if left {
        chars.next();
    }
    let mut dashes = 0;
    while chars.peek() == Some(&'-') {
        chars.next();
        dashes += 1;
    }
    let right = chars.peek() == Some(&':');
    if right {
        chars.next();
    }
    if dashes == 0 || chars.next().is_some() {
        return None;
    }
    Some(match (left, right) {
        (true, true) => Alignment::AlignCenter,
        (true, false) => Alignment::AlignLeft,
        (false, true) => Alignment::AlignRight,
        (false, false) => Alignment::AlignDefault,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(line: &str, code_spans: bool) -> Vec<String> {
        split_cells(line, code_spans).0
    }

    fn pipe_row(line: &str, code_spans: bool) -> bool {
        let (cells, pipes) = split_cells(line, code_spans);
        is_pipe_row(&cells, pipes)
    }

    #[test]
    fn edge_pipes_do_not_make_cells_but_doubled_ones_do() {
        assert_eq!(cells("| a | b |", false), ["a", "b"]);
        assert_eq!(cells("a | b", false), ["a", "b"]);
        assert_eq!(cells("| a | b", false), ["a", "b"]);
        assert_eq!(cells("a | b |", false), ["a", "b"]);
        assert_eq!(cells("||b|", false), ["", "b"]);
        assert_eq!(cells("|a|b||", false), ["a", "b", ""]);
    }

    #[test]
    fn structural_pipe_count_includes_edges_and_separators() {
        assert_eq!(split_cells("| a | b |", false).1, 3);
        assert_eq!(split_cells("a | b", false).1, 1);
        assert_eq!(split_cells("||b|", false).1, 3);
        // Escaped and code-span pipes are not structural.
        assert_eq!(split_cells("x\\|y | z", false).1, 1);
        assert_eq!(split_cells("`a|b` | c", true).1, 1);
    }

    #[test]
    fn escaped_pipe_is_literal() {
        assert_eq!(cells("x\\|y | z", false), ["x|y", "z"]);
    }

    #[test]
    fn a_code_span_pipe_splits_the_row_only_without_code_span_awareness() {
        // A closed backtick span keeps a pipe whole when code spans are honored; otherwise every
        // unescaped pipe splits, so the span is torn across two cells.
        assert_eq!(cells("`a|b` | c", true), ["`a|b`", "c"]);
        assert_eq!(cells("`a|b` | c", false), ["`a", "b`", "c"]);
        // A doubled backtick run is a span too; an unclosed run is not, so its pipe still splits.
        assert_eq!(cells("``x|y`` | z", true), ["``x|y``", "z"]);
        assert_eq!(cells("`a | b", true), ["`a", "b"]);
        // Only pipes strictly inside the span are protected.
        assert_eq!(cells("`x` | `y|z`", true), ["`x`", "`y|z`"]);
    }

    #[test]
    fn multibyte_cell_content_survives_byte_offset_scanning() {
        // CJK, emoji, and combining marks sit directly against the separators; the byte scan must
        // slice on the ASCII pipe boundaries without splitting a multibyte character.
        assert_eq!(cells("| 表格 | 数据 |", false), ["表格", "数据"]);
        assert_eq!(cells("a😀 | b🎉", false), ["a😀", "b🎉"]);
        assert_eq!(
            cells("| e\u{0301} | o\u{0308} |", false),
            ["e\u{0301}", "o\u{0308}"]
        );
        // An escaped pipe beside multibyte content still drops only the backslash.
        assert_eq!(cells("表\\|格 | 数", false), ["表|格", "数"]);
        // A code span holding a multibyte, pipe-bearing payload stays one cell.
        assert_eq!(cells("`表|格` | 数", true), ["`表|格`", "数"]);
    }

    #[test]
    fn delimiter_alignment_from_colons() {
        assert_eq!(
            parse_delimiter("| :-- | :-: | --: |", false),
            Some(vec![
                Alignment::AlignLeft,
                Alignment::AlignCenter,
                Alignment::AlignRight,
            ])
        );
        assert_eq!(
            parse_delimiter("| - | --- |", false),
            Some(vec![Alignment::AlignDefault, Alignment::AlignDefault])
        );
    }

    #[test]
    fn invalid_delimiter_cells_reject_the_row() {
        assert_eq!(parse_delimiter("| ::: | --- |", false), None);
        assert_eq!(parse_delimiter("| :: | - |", false), None);
        assert_eq!(parse_delimiter("| - - | - |", false), None);
        assert_eq!(parse_delimiter("| x | - |", false), None);
        assert_eq!(parse_delimiter("|  | - |", false), None);
    }

    #[test]
    fn column_count_must_match_header() {
        assert!(try_parse("| a | b | c |\n| - | - |\n", false).is_none());
        assert!(try_parse("| a | b |\n| - | - | - |\n", false).is_none());
    }

    #[test]
    fn body_rows_are_padded_and_truncated() {
        let (_, _, rows) =
            try_parse("| a | b |\n| - | - |\n| 1 |\n| 1 | 2 | 3 |\n", false).unwrap();
        assert_eq!(rows, vec![vec!["1", ""], vec!["1", "2"]]);
    }

    #[test]
    fn a_code_span_cell_keeps_its_pipe_and_one_column() {
        // With code spans honored the body cell is a single column holding the whole span; without,
        // the extra pipe splits it and the surplus cell is truncated away.
        let (_, _, rows) = try_parse("| h |\n| - |\n| `a|b` |\n", true).unwrap();
        assert_eq!(rows, vec![vec!["`a|b`"]]);
        // A line whose only pipe sits inside a span is not a pipe row, so it opens no table.
        assert!(try_parse("`x|y`\n| - |\n", true).is_none());
    }

    #[test]
    fn header_and_delimiter_only_is_a_valid_empty_body() {
        let (aligns, header, rows) = try_parse("| a | b |\n| - | - |\n", false).unwrap();
        assert_eq!(aligns.len(), 2);
        assert_eq!(header, ["a", "b"]);
        assert!(rows.is_empty());
    }

    #[test]
    fn a_lone_pipe_row_is_not_a_table() {
        assert!(try_parse("| a | b |\n", false).is_none());
    }

    #[test]
    fn lone_pipe_is_not_a_pipe_row() {
        assert!(!pipe_row("|", false));
        assert!(!pipe_row("   ", false));
        assert!(!pipe_row("plain", false));
        assert!(pipe_row("||", false));
        assert!(pipe_row("| |", false));
        assert!(pipe_row("a | b", false));
        assert!(pipe_row("| a", false));
        // A pipe held inside a code span is not cell structure.
        assert!(!pipe_row("`a|b`", true));
    }

    #[test]
    fn header_only_does_not_absorb_a_list_marker_delimiter() {
        // With just a header present the candidate delimiter is left to the ordinary block rules,
        // so a `- |` delimiter opens a list rather than being claimed as a table.
        assert!(matches!(
            classify_continuation("| a | b |\n", "- | -", false, false),
            Continuation::NotTable
        ));
    }

    #[test]
    fn established_table_absorbs_pipe_rows_and_ends_on_others() {
        let paragraph = "| a | b |\n| - | - |\n";
        assert!(matches!(
            classify_continuation(paragraph, "- | 1", false, false),
            Continuation::Absorb
        ));
        assert!(matches!(
            classify_continuation(paragraph, "plain text", false, false),
            Continuation::Terminate
        ));
        // Once established the header/delimiter are not re-inspected: an unparsable prefix in the
        // paragraph slot is ignored and only the new line decides.
        assert!(matches!(
            classify_continuation("", "| 1 | 2 |", false, true),
            Continuation::Absorb
        ));
        assert!(matches!(
            classify_continuation("", "plain text", false, true),
            Continuation::Terminate
        ));
    }
}
