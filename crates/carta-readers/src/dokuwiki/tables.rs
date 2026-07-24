//! Table row parsing, column alignment, and delimiter-aware cell splitting.

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Row, Table, TableBody, TableFoot,
    TableHead,
};

use super::Ctx;
use super::helpers::{find_subsequence, matches_at};
use super::inline::inline_content;

/// Whether the line opens a table row: it begins with a cell delimiter and yields at least one cell.
/// A lone delimiter with nothing to delimit is ordinary text, not a degenerate one-row table.
pub(super) fn is_table_line(line: &str) -> bool {
    (line.starts_with('|') || line.starts_with('^')) && !split_row(line).is_empty()
}

/// Parse a run of table rows. The first row sets the column count and per-column alignment, and is
/// the header row when it opens with `^`; all remaining rows form the single body.
pub(super) fn parse_table(lines: &[&str], index: &mut usize, ctx: Ctx, depth: usize) -> Block {
    let mut rows: Vec<(bool, Vec<String>)> = Vec::new();
    while *index < lines.len() {
        let line = lines.get(*index).copied().unwrap_or("");
        if !is_table_line(line) {
            break;
        }
        rows.push((line.starts_with('^'), split_row(line)));
        *index += 1;
    }

    let first = rows.first();
    let col_count = first.map_or(0, |(_, cells)| cells.len());
    let col_specs: Vec<ColSpec> = first
        .map(|(_, cells)| {
            cells
                .iter()
                .map(|cell| ColSpec {
                    align: cell_align(cell),
                    width: ColWidth::ColWidthDefault,
                })
                .collect()
        })
        .unwrap_or_default();

    let mut head_rows = Vec::new();
    let mut body_rows = Vec::new();
    for (i, (header, cells)) in rows.iter().enumerate() {
        let row = build_row(cells, col_count, ctx, depth);
        if i == 0 && *header {
            head_rows.push(row);
        } else {
            body_rows.push(row);
        }
    }

    Block::Table(Box::new(Table {
        attr: Attr::default(),
        caption: Caption::default(),
        col_specs,
        head: TableHead {
            attr: Attr::default(),
            rows: head_rows,
        },
        bodies: vec![TableBody {
            attr: Attr::default(),
            row_head_columns: 0,
            head: Vec::new(),
            body: body_rows,
        }],
        foot: TableFoot::default(),
    }))
}

/// Build a table row, fitting it to `col_count` by truncating extra cells and padding short rows.
fn build_row(cells: &[String], col_count: usize, ctx: Ctx, depth: usize) -> Row {
    let mut out = Vec::with_capacity(col_count);
    for i in 0..col_count {
        let trimmed = cells.get(i).map_or("", |c| c.trim());
        let content = if trimmed.is_empty() {
            Vec::new()
        } else {
            vec![Block::Plain(inline_content(trimmed, ctx, depth))]
        };
        out.push(Cell {
            attr: Attr::default(),
            align: Alignment::AlignDefault,
            row_span: 1,
            col_span: 1,
            content,
        });
    }
    Row {
        attr: Attr::default(),
        cells: out,
    }
}

/// The column alignment implied by a raw cell's padding: at least two spaces on a side anchors that
/// side, both anchors centre, neither leaves the default.
fn cell_align(raw: &str) -> Alignment {
    let leading = raw.chars().take_while(|&c| c == ' ').count();
    let trailing = raw.chars().rev().take_while(|&c| c == ' ').count();
    match (leading >= 2, trailing >= 2) {
        (true, true) => Alignment::AlignCenter,
        (_, true) => Alignment::AlignLeft,
        (true, _) => Alignment::AlignRight,
        _ => Alignment::AlignDefault,
    }
}

/// Split a table row into its raw cell texts, treating `|` and `^` as delimiters but ignoring those
/// inside links, media, monospace, no-format spans, and verbatim regions.
fn split_row(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut segments: Vec<String> = Vec::new();
    let mut seg = String::new();
    let mut i = 0;
    while i < chars.len() {
        if let Some(skip) = protected_end(&chars, i) {
            seg.extend(chars.get(i..skip).unwrap_or(&[]));
            i = skip;
            continue;
        }
        match chars.get(i) {
            Some('|' | '^') => {
                segments.push(std::mem::take(&mut seg));
                i += 1;
            }
            Some(&c) => {
                seg.push(c);
                i += 1;
            }
            None => break,
        }
    }
    segments.push(seg);
    if !segments.is_empty() {
        segments.remove(0);
    }
    if segments.last().is_some_and(String::is_empty) {
        segments.pop();
    }
    segments
}

/// If a protected span opens at `i`, the index just past its closing delimiter (or the end of the
/// line when it is unterminated).
fn protected_end(chars: &[char], i: usize) -> Option<usize> {
    for (open, close) in [("[[", "]]"), ("{{", "}}"), ("''", "''"), ("%%", "%%")] {
        if matches_at(chars, i, open) {
            let from = i + open.chars().count();
            let end = find_subsequence(chars, from, close)
                .map_or(chars.len(), |p| p + close.chars().count());
            return Some(end);
        }
    }
    if matches_at(chars, i, "<nowiki>") {
        let from = i + "<nowiki>".chars().count();
        let end = find_subsequence(chars, from, "</nowiki>")
            .map_or(chars.len(), |p| p + "</nowiki>".chars().count());
        return Some(end);
    }
    None
}
