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
pub(crate) fn try_parse(text: &str) -> Option<ParsedTable> {
    let mut lines = text.lines();
    let header_line = lines.next()?;
    let delimiter_line = lines.next()?;

    let header = split_cells(header_line);
    if !is_pipe_row(header_line, &header) {
        return None;
    }
    let alignments = parse_delimiter(delimiter_line)?;
    if header.len() != alignments.len() {
        return None;
    }
    let columns = alignments.len();

    let rows = lines
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let mut cells = split_cells(line);
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
pub(crate) fn classify_continuation(paragraph: &str, line: &str) -> Continuation {
    let mut existing = paragraph.lines();
    let Some(header_line) = existing.next() else {
        return Continuation::NotTable;
    };
    let Some(delimiter_line) = existing.next() else {
        return Continuation::NotTable;
    };
    let header = split_cells(header_line);
    if !is_pipe_row(header_line, &header) {
        return Continuation::NotTable;
    }
    let aligned = matches!(
        parse_delimiter(delimiter_line),
        Some(alignments) if alignments.len() == header.len()
    );
    if !aligned {
        return Continuation::NotTable;
    }
    if is_pipe_row(line, &split_cells(line)) {
        Continuation::Absorb
    } else {
        Continuation::Terminate
    }
}

/// Split one table row into its trimmed cell texts.
///
/// A single leading or trailing `|` is an edge delimiter and yields no cell; a doubled edge pipe
/// (or an interior `||`) yields an empty cell. A backslash-escaped `\|` is a literal pipe within a
/// cell, not a split point.
fn split_cells(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    let mut chars = trimmed.chars().peekable();
    if chars.peek() == Some(&'|') {
        chars.next();
    }

    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut ended_on_pipe = false;
    while let Some(ch) = chars.next() {
        match ch {
            '\\' if chars.peek() == Some(&'|') => {
                chars.next();
                cell.push('|');
                ended_on_pipe = false;
            }
            '|' => {
                cells.push(std::mem::take(&mut cell));
                ended_on_pipe = true;
            }
            other => {
                cell.push(other);
                ended_on_pipe = false;
            }
        }
    }
    cells.push(cell);
    if ended_on_pipe {
        cells.pop();
    }

    cells.iter().map(|cell| cell.trim().to_owned()).collect()
}

/// Parse a delimiter row into its per-column alignments, or `None` if it is not a pipe row or any
/// cell is not a valid `:?-+:?` run.
fn parse_delimiter(line: &str) -> Option<Vec<Alignment>> {
    let cells = split_cells(line);
    if !is_pipe_row(line, &cells) {
        return None;
    }
    cells.iter().map(|cell| delimiter_align(cell)).collect()
}

/// Whether a line carries enough pipe structure to be a table header or delimiter row. A row needs
/// two or more pipes, or a single pipe alongside some non-empty cell; a lone `|` or a pipeless line
/// is not a row.
fn is_pipe_row(line: &str, cells: &[String]) -> bool {
    let pipes = unescaped_pipe_count(line.trim());
    pipes >= 2 || (pipes >= 1 && cells.iter().any(|cell| !cell.is_empty()))
}

/// Count the pipes in a row that act as cell separators — every `|` except those escaped as `\|`.
fn unescaped_pipe_count(text: &str) -> usize {
    let mut chars = text.chars().peekable();
    let mut count = 0;
    while let Some(ch) = chars.next() {
        match ch {
            '\\' if chars.peek() == Some(&'|') => {
                chars.next();
            }
            '|' => count += 1,
            _ => {}
        }
    }
    count
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

    #[test]
    fn edge_pipes_do_not_make_cells_but_doubled_ones_do() {
        assert_eq!(split_cells("| a | b |"), ["a", "b"]);
        assert_eq!(split_cells("a | b"), ["a", "b"]);
        assert_eq!(split_cells("| a | b"), ["a", "b"]);
        assert_eq!(split_cells("a | b |"), ["a", "b"]);
        assert_eq!(split_cells("||b|"), ["", "b"]);
        assert_eq!(split_cells("|a|b||"), ["a", "b", ""]);
    }

    #[test]
    fn escaped_pipe_is_literal() {
        assert_eq!(split_cells("x\\|y | z"), ["x|y", "z"]);
    }

    #[test]
    fn delimiter_alignment_from_colons() {
        assert_eq!(
            parse_delimiter("| :-- | :-: | --: |"),
            Some(vec![
                Alignment::AlignLeft,
                Alignment::AlignCenter,
                Alignment::AlignRight,
            ])
        );
        assert_eq!(
            parse_delimiter("| - | --- |"),
            Some(vec![Alignment::AlignDefault, Alignment::AlignDefault])
        );
    }

    #[test]
    fn invalid_delimiter_cells_reject_the_row() {
        assert_eq!(parse_delimiter("| ::: | --- |"), None);
        assert_eq!(parse_delimiter("| :: | - |"), None);
        assert_eq!(parse_delimiter("| - - | - |"), None);
        assert_eq!(parse_delimiter("| x | - |"), None);
        assert_eq!(parse_delimiter("|  | - |"), None);
    }

    #[test]
    fn column_count_must_match_header() {
        assert!(try_parse("| a | b | c |\n| - | - |\n").is_none());
        assert!(try_parse("| a | b |\n| - | - | - |\n").is_none());
    }

    #[test]
    fn body_rows_are_padded_and_truncated() {
        let (_, _, rows) = try_parse("| a | b |\n| - | - |\n| 1 |\n| 1 | 2 | 3 |\n").unwrap();
        assert_eq!(rows, vec![vec!["1", ""], vec!["1", "2"]]);
    }

    #[test]
    fn header_and_delimiter_only_is_a_valid_empty_body() {
        let (aligns, header, rows) = try_parse("| a | b |\n| - | - |\n").unwrap();
        assert_eq!(aligns.len(), 2);
        assert_eq!(header, ["a", "b"]);
        assert!(rows.is_empty());
    }

    #[test]
    fn a_lone_pipe_row_is_not_a_table() {
        assert!(try_parse("| a | b |\n").is_none());
    }

    #[test]
    fn lone_pipe_is_not_a_pipe_row() {
        assert!(!is_pipe_row("|", &split_cells("|")));
        assert!(!is_pipe_row("   ", &split_cells("   ")));
        assert!(!is_pipe_row("plain", &split_cells("plain")));
        assert!(is_pipe_row("||", &split_cells("||")));
        assert!(is_pipe_row("| |", &split_cells("| |")));
        assert!(is_pipe_row("a | b", &split_cells("a | b")));
        assert!(is_pipe_row("| a", &split_cells("| a")));
    }

    #[test]
    fn header_only_does_not_absorb_a_list_marker_delimiter() {
        // With just a header present the candidate delimiter is left to the ordinary block rules,
        // so a `- |` delimiter opens a list rather than being claimed as a table.
        assert!(matches!(
            classify_continuation("| a | b |\n", "- | -"),
            Continuation::NotTable
        ));
    }

    #[test]
    fn established_table_absorbs_pipe_rows_and_ends_on_others() {
        let paragraph = "| a | b |\n| - | - |\n";
        assert!(matches!(
            classify_continuation(paragraph, "- | 1"),
            Continuation::Absorb
        ));
        assert!(matches!(
            classify_continuation(paragraph, "plain text"),
            Continuation::Terminate
        ));
    }
}
