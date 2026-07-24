//! Parsing of `tbl` table regions into table blocks.

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Inline, Row, Table, TableBody,
    TableFoot, TableHead,
};

use super::inline::read_delimited;

/// Builds a [`Block::Table`] from the lines of a tbl region (those between `.TS` and `.TE`, both
/// excluded). The region is the preprocessor's: an optional options line ending in `;` (carrying the
/// cell separator in its `tab(X)` option), one or more format lines the last of which ends in `.`
/// (the first fixes the column count and alignments), then the data rows. A rule line (`_`/`=`) just
/// below the first data row promotes that row to the table head. A `T{`…`T}` text block spanning
/// several input lines collapses into one filled cell. A format declaring a horizontal span, which
/// the table model cannot express, renders as a placeholder paragraph. Returns `None` for a region
/// with no format line, where there is no table to build.
pub(super) fn build_tbl(region: &[String]) -> Option<Block> {
    let mut index = 0;
    let mut separator = "\t".to_owned();
    if let Some(first) = region.first()
        && first.trim_end().ends_with(';')
    {
        if let Some(sep) = tab_option(first) {
            separator = sep;
        }
        index = 1;
    }

    let aligns = parse_col_aligns(region.get(index)?);
    if aligns.is_empty() {
        return None;
    }
    let columns = aligns.len();
    let mut data_start = None;
    for (offset, line) in region.iter().enumerate().skip(index) {
        if line.trim_end().ends_with('.') {
            data_start = Some(offset + 1);
            break;
        }
    }
    let data_start = data_start?;

    // Horizontal column spans are unrepresentable; such regions render as a placeholder paragraph.
    if region
        .get(index..data_start)
        .unwrap_or(&[])
        .iter()
        .any(|line| format_has_span(line))
    {
        return Some(Block::Para(vec![Inline::Str("TABLE".into())]));
    }

    let data = collapse_text_blocks(region.get(data_start..).unwrap_or(&[]), &separator);

    let (head_lines, body_lines): (&[String], &[String]) =
        if data.get(1).is_some_and(|line| is_rule(line)) {
            (data.get(..1).unwrap_or(&[]), data.get(2..).unwrap_or(&[]))
        } else {
            (&[], &data)
        };

    let col_specs = aligns
        .into_iter()
        .map(|align| ColSpec {
            align,
            width: ColWidth::ColWidthDefault,
        })
        .collect();
    let head = TableHead {
        attr: Attr::default(),
        rows: head_lines
            .iter()
            .map(|line| tbl_row(line, &separator, columns))
            .collect(),
    };
    let body = TableBody {
        attr: Attr::default(),
        row_head_columns: 0,
        head: Vec::new(),
        body: body_lines
            .iter()
            .filter(|line| !is_rule(line))
            .map(|line| tbl_row(line, &separator, columns))
            .collect(),
    };

    Some(Block::Table(Box::new(Table {
        attr: Attr::default(),
        caption: Caption::default(),
        col_specs,
        head,
        bodies: vec![body],
        foot: TableFoot::default(),
    })))
}

/// Reads the cell separator from a tbl options line's `tab(X)` option, if it carries one.
fn tab_option(options: &str) -> Option<String> {
    let inside = options.split_once("tab(")?.1.split_once(')')?.0;
    (!inside.is_empty()).then(|| inside.to_owned())
}

/// Parses the alignment of each column from a tbl format line. Each key letter (`l`/`a` left, `r`/`n`
/// right, `c` center) opens a column; `s` continues a horizontal span; a font modifier (`f` and its
/// name) and a width modifier (`w`/`p`/`v`/`m` and its parenthesized or numeric argument) are skipped.
fn parse_col_aligns(spec: &str) -> Vec<Alignment> {
    let mut aligns = Vec::new();
    let mut chars = spec.chars().peekable();
    while let Some(c) = chars.next() {
        match c.to_ascii_lowercase() {
            'l' | 'a' => aligns.push(Alignment::AlignLeft),
            'r' | 'n' => aligns.push(Alignment::AlignRight),
            'c' => aligns.push(Alignment::AlignCenter),
            'f' => match chars.peek() {
                Some('(') => {
                    chars.next();
                    chars.next();
                    chars.next();
                }
                Some('[') => {
                    chars.next();
                    read_delimited(&mut chars, ']');
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            },
            'w' | 'p' | 'v' | 'm' => {
                if chars.peek() == Some(&'(') {
                    chars.next();
                    for d in chars.by_ref() {
                        if d == ')' {
                            break;
                        }
                    }
                } else {
                    while matches!(chars.peek(), Some(d) if d.is_ascii_digit()) {
                        chars.next();
                    }
                }
            }
            _ => {}
        }
    }
    aligns
}

/// Whether a tbl format line declares a horizontal span (an `s`/`S` key), skipping the font and width
/// modifiers whose own arguments could otherwise contain that letter.
fn format_has_span(spec: &str) -> bool {
    let mut chars = spec.chars().peekable();
    while let Some(c) = chars.next() {
        match c.to_ascii_lowercase() {
            's' => return true,
            'f' => match chars.peek() {
                Some('(') => {
                    chars.next();
                    chars.next();
                    chars.next();
                }
                Some('[') => {
                    chars.next();
                    read_delimited(&mut chars, ']');
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            },
            'w' | 'p' | 'v' | 'm' => {
                if chars.peek() == Some(&'(') {
                    chars.next();
                    for d in chars.by_ref() {
                        if d == ')' {
                            break;
                        }
                    }
                } else {
                    while matches!(chars.peek(), Some(d) if d.is_ascii_digit()) {
                        chars.next();
                    }
                }
            }
            _ => {}
        }
    }
    false
}

/// Collapses tbl text blocks into single data lines. A field of `T{` begins a block whose content is
/// the following lines up to a line starting with `T}`; those lines join with single spaces into the
/// field, and any fields after `T}` on its line continue the row.
fn collapse_text_blocks(data: &[String], separator: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut index = 0;
    while let Some(line) = data.get(index) {
        index += 1;
        if !line.split(separator).any(|field| field.trim() == "T{") {
            out.push(line.clone());
            continue;
        }
        let mut fields: Vec<String> = Vec::new();
        for field in line.split(separator) {
            if field.trim() != "T{" {
                fields.push(field.to_owned());
                continue;
            }
            let mut block: Vec<String> = Vec::new();
            let mut terminated = false;
            while let Some(block_line) = data.get(index) {
                index += 1;
                if block_line.trim_start().starts_with("T}") {
                    let mut tail = block_line.split(separator);
                    tail.next();
                    fields.push(block.join(" "));
                    fields.extend(tail.map(str::to_owned));
                    terminated = true;
                    break;
                }
                block.push(block_line.clone());
            }
            if !terminated {
                fields.push(block.join(" "));
            }
        }
        out.push(fields.join(separator));
    }
    out
}

/// Whether a tbl line is a horizontal rule: a non-empty line of only `_` or `=` characters.
fn is_rule(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty() && trimmed.chars().all(|c| c == '_' || c == '=')
}

/// Builds one table row of exactly `columns` cells from a tbl data line: fields past the column count
/// are dropped and missing fields are filled with empty cells.
fn tbl_row(line: &str, separator: &str, columns: usize) -> Row {
    let mut cells: Vec<Cell> = line.split(separator).take(columns).map(tbl_cell).collect();
    while cells.len() < columns {
        cells.push(tbl_cell(""));
    }
    Row {
        attr: Attr::default(),
        cells,
    }
}

/// Builds a table cell from raw field text: surviving backslash escapes are stripped and the
/// remainder is split on whitespace into words. An empty field yields a cell with no content.
fn tbl_cell(field: &str) -> Cell {
    let cleaned: String = field.chars().filter(|&c| c != '\\').collect();
    let mut inlines = Vec::new();
    for word in cleaned.split_whitespace() {
        if !inlines.is_empty() {
            inlines.push(Inline::Space);
        }
        inlines.push(Inline::Str(word.into()));
    }
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
