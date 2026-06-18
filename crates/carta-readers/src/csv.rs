//! Delimiter-separated value readers (CSV and TSV).
//!
//! Both formats render to a single [`Block::Table`]: the first record becomes the table head and
//! every later record a body row. The first record also fixes the column count — wider records are
//! truncated, narrower ones padded with empty cells.
//!
//! The two formats differ only in how records split into fields. CSV uses a comma delimiter and
//! honors double-quote quoting (a quoted field may contain the delimiter, line breaks, and `""`
//! escapes for a literal quote). TSV uses a tab delimiter and has no quoting: every byte is literal
//! and a line break always ends a record. Both share the field-to-inlines tokenizer below.

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Document, Inline, Row, Table,
    TableBody, TableFoot, TableHead,
};
use carta_core::{Reader, ReaderOptions, Result};

/// Parses comma-separated values into a single table.
#[derive(Debug, Default, Clone, Copy)]
pub struct CsvReader;

impl Reader for CsvReader {
    fn read(&self, input: &str, _options: &ReaderOptions) -> Result<Document> {
        Ok(build_document(parse_records(input, ',', true)))
    }
}

/// Splits delimiter-separated input into records of fields. With `quoting` enabled a field may be
/// double-quoted, in which case the delimiter and line breaks are literal and `""` denotes a single
/// quote; otherwise every character is literal and a line break always ends the record.
pub(crate) fn parse_records(input: &str, delimiter: char, quoting: bool) -> Vec<Vec<String>> {
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut field = String::new();
    let mut chars = input
        .strip_prefix('\u{feff}')
        .unwrap_or(input)
        .chars()
        .peekable();

    loop {
        match chars.next() {
            None => break,
            Some('"') if quoting && field.is_empty() => {
                read_quoted_field(&mut chars, &mut field);
            }
            Some(c) if c == delimiter => {
                record.push(std::mem::take(&mut field));
                skip_leading_blanks(&mut chars, delimiter);
            }
            Some('\r') => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                record.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut record));
            }
            Some('\n') => {
                record.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut record));
            }
            Some(c) => field.push(c),
        }
    }

    if !field.is_empty() || !record.is_empty() {
        record.push(field);
        records.push(record);
    }

    records
}

/// Skips the spaces and tabs that lead a field, stopping at the delimiter itself so a delimiter is
/// never consumed as padding.
fn skip_leading_blanks(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, delimiter: char) {
    while let Some(&c) = chars.peek() {
        if (c == ' ' || c == '\t') && c != delimiter {
            chars.next();
        } else {
            break;
        }
    }
}

/// Consumes a double-quoted field body, having already passed the opening quote. A doubled quote
/// inside the body is a literal quote; the first lone quote closes the field.
fn read_quoted_field(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, field: &mut String) {
    while let Some(c) = chars.next() {
        if c == '"' {
            if chars.peek() == Some(&'"') {
                chars.next();
                field.push('"');
            } else {
                return;
            }
        } else {
            field.push(c);
        }
    }
}

/// Assembles parsed records into a one-table document. An input with no records yields an empty
/// document.
pub(crate) fn build_document(records: Vec<Vec<String>>) -> Document {
    let mut records = records.into_iter();
    let Some(header) = records.next() else {
        return Document::default();
    };

    let column_count = header.len();
    let col_specs = (0..column_count)
        .map(|_| ColSpec {
            align: Alignment::AlignDefault,
            width: ColWidth::ColWidthDefault,
        })
        .collect();

    let head = TableHead {
        attr: Attr::default(),
        rows: vec![field_row(header, column_count)],
    };
    let body_rows = records
        .map(|record| field_row(record, column_count))
        .collect();
    let body = TableBody {
        attr: Attr::default(),
        row_head_columns: 0,
        head: Vec::new(),
        body: body_rows,
    };

    let table = Table {
        attr: Attr::default(),
        caption: Caption::default(),
        col_specs,
        head,
        bodies: vec![body],
        foot: TableFoot::default(),
    };

    Document {
        blocks: vec![Block::Table(Box::new(table))],
        ..Default::default()
    }
}

/// Builds one table row of exactly `column_count` cells: extra fields are dropped, missing fields
/// are added as empty cells.
fn field_row(fields: Vec<String>, column_count: usize) -> Row {
    let mut cells: Vec<Cell> = fields
        .into_iter()
        .take(column_count)
        .map(|field| field_cell(&field))
        .collect();
    while cells.len() < column_count {
        cells.push(field_cell(""));
    }
    Row {
        attr: Attr::default(),
        cells,
    }
}

fn field_cell(field: &str) -> Cell {
    let inlines = field_inlines(field);
    let content = if inlines.is_empty() {
        Vec::new()
    } else {
        vec![Block::Plain(inlines)]
    };
    Cell {
        attr: Attr::default(),
        align: Alignment::AlignDefault,
        row_span: 1,
        col_span: 1,
        content,
    }
}

/// Tokenizes a field's text into inlines. Carriage returns are dropped, a single trailing line feed
/// is discarded as a record terminator artifact, runs of non-newline whitespace become a single
/// [`Inline::Space`], and each remaining line feed becomes an [`Inline::LineBreak`].
fn field_inlines(field: &str) -> Vec<Inline> {
    let cleaned: String = field.chars().filter(|&c| c != '\r').collect();
    let cleaned = match cleaned.strip_suffix('\n') {
        Some(trimmed) => trimmed,
        None => &cleaned,
    };

    let mut inlines = Vec::new();
    let mut chars = cleaned.chars().peekable();
    while let Some(&c) = chars.peek() {
        if is_separator(c) {
            let mut newlines = 0;
            while let Some(&w) = chars.peek() {
                if w == '\n' {
                    newlines += 1;
                    chars.next();
                } else if is_separator(w) {
                    chars.next();
                } else {
                    break;
                }
            }
            if newlines == 0 {
                inlines.push(Inline::Space);
            } else {
                for _ in 0..newlines {
                    inlines.push(Inline::LineBreak);
                }
            }
        } else {
            let mut word = String::new();
            while let Some(&w) = chars.peek() {
                if is_separator(w) {
                    break;
                }
                word.push(w);
                chars.next();
            }
            inlines.push(Inline::Str(word));
        }
    }

    inlines
}

/// The field tokenizer treats only ASCII space, tab, and line feed as separators; every other
/// character (including other Unicode whitespace) is part of a word.
fn is_separator(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(inlines: &[Inline]) -> Vec<&'static str> {
        inlines
            .iter()
            .map(|inline| match inline {
                Inline::Str(_) => "Str",
                Inline::Space => "Space",
                Inline::LineBreak => "LineBreak",
                _ => "other",
            })
            .collect()
    }

    #[test]
    fn collapses_whitespace_runs_to_single_space() {
        assert_eq!(tags(&field_inlines("x  y")), ["Str", "Space", "Str"]);
        assert_eq!(tags(&field_inlines("x\ty")), ["Str", "Space", "Str"]);
    }

    #[test]
    fn keeps_leading_and_trailing_space_around_words() {
        assert_eq!(tags(&field_inlines(" x ")), ["Space", "Str", "Space"]);
    }

    #[test]
    fn pure_whitespace_field_is_one_space() {
        assert_eq!(tags(&field_inlines("   ")), ["Space"]);
    }

    #[test]
    fn embedded_newlines_become_line_breaks() {
        assert_eq!(tags(&field_inlines("x\ny")), ["Str", "LineBreak", "Str"]);
        assert_eq!(
            tags(&field_inlines("x\n\ny")),
            ["Str", "LineBreak", "LineBreak", "Str"]
        );
    }

    #[test]
    fn single_trailing_newline_is_dropped() {
        assert!(field_inlines("\n").is_empty());
        assert_eq!(tags(&field_inlines(" \n")), ["Space"]);
        assert_eq!(tags(&field_inlines("\n ")), ["LineBreak"]);
    }

    #[test]
    fn carriage_returns_are_removed() {
        assert_eq!(tags(&field_inlines("x\ry")), ["Str"]);
        assert_eq!(tags(&field_inlines("x\r\ny")), ["Str", "LineBreak", "Str"]);
    }

    #[test]
    fn non_ascii_whitespace_stays_in_word() {
        assert_eq!(tags(&field_inlines("x\u{a0}y")), ["Str"]);
    }

    #[test]
    fn quoting_protects_delimiter_and_escapes_quote() {
        let records = parse_records("\"a,b\",\"c\"\"d\"\n", ',', true);
        assert_eq!(records, vec![vec!["a,b".to_owned(), "c\"d".to_owned()]]);
    }

    #[test]
    fn tab_records_keep_quotes_literal() {
        let records = parse_records("\"a\"\tb\n", '\t', false);
        assert_eq!(records, vec![vec!["\"a\"".to_owned(), "b".to_owned()]]);
    }

    #[test]
    fn leading_blanks_after_delimiter_are_skipped() {
        let records = parse_records("a,  b,\tc\n", ',', true);
        assert_eq!(
            records,
            vec![vec!["a".to_owned(), "b".to_owned(), "c".to_owned()]]
        );
    }

    #[test]
    fn first_field_keeps_leading_blanks() {
        let records = parse_records(" a,b\n", ',', true);
        assert_eq!(records, vec![vec![" a".to_owned(), "b".to_owned()]]);
    }

    #[test]
    fn crlf_and_bare_lf_both_end_records() {
        let records = parse_records("a,b\r\nc,d\ne,f", ',', true);
        assert_eq!(records.len(), 3);
    }

    #[test]
    fn empty_input_yields_empty_document() {
        assert!(
            build_document(parse_records("", ',', true))
                .blocks
                .is_empty()
        );
    }

    #[test]
    fn leading_byte_order_mark_is_stripped() {
        let records = parse_records("\u{feff}a,b\n", ',', true);
        assert_eq!(records, vec![vec!["a".to_owned(), "b".to_owned()]]);
    }
}
