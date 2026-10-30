//! Parsing of `tbl` table regions into table blocks.

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Inline, Row, Table, TableBody,
    TableFoot, TableHead,
};

use super::inline::read_delimited;

enum ColumnFormat {
    Aligned(Alignment),
    HorizontalSpan,
    VerticalSpan,
}

/// Builds a table from its format descriptions and data rows.
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
    let formats = read_formats(region, &mut index)?;
    let columns = formats.iter().map(Vec::len).max()?;
    let col_specs: Vec<_> = (0..columns)
        .map(|column| ColSpec {
            align: formats
                .iter()
                .find_map(|format| match format.get(column) {
                    Some(ColumnFormat::Aligned(alignment)) => Some(alignment.clone()),
                    _ => None,
                })
                .unwrap_or(Alignment::AlignDefault),
            width: ColWidth::ColWidthDefault,
        })
        .collect();
    let data = collapse_text_blocks(region.get(index..).unwrap_or(&[]), &separator);
    let (mut rows, header) = build_rows(&data, &separator, formats, &col_specs);
    let (head_rows, body_rows) = if header && !rows.is_empty() {
        let body = rows.split_off(1);
        (rows, body)
    } else {
        (Vec::new(), rows)
    };
    Some(Block::Table(Box::new(Table {
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
    })))
}

fn read_formats(lines: &[String], index: &mut usize) -> Option<Vec<Vec<ColumnFormat>>> {
    let mut formats = Vec::new();
    while let Some(line) = lines.get(*index) {
        *index += 1;
        formats.extend(parse_column_formats(line));
        if line.trim_end().ends_with('.') {
            return (!formats.is_empty()).then_some(formats);
        }
    }
    None
}

fn build_rows(
    data: &[String],
    separator: &str,
    mut formats: Vec<Vec<ColumnFormat>>,
    columns: &[ColSpec],
) -> (Vec<Row>, bool) {
    let mut rows: Vec<Row> = Vec::new();
    let mut anchors: Vec<Option<(usize, usize)>> = vec![None; columns.len()];
    let mut header = false;
    let mut index = 0;
    let mut format_index = 0;
    while let Some(line) = data.get(index) {
        index += 1;
        if line.trim() == ".T&" {
            if let Some(next) = read_formats(data, &mut index) {
                formats = next;
                format_index = 0;
            }
            continue;
        }
        if is_rule(line) {
            if rows.len() == 1 {
                header = true;
                anchors.fill(None);
            }
            continue;
        }
        if line.starts_with('.') {
            continue;
        }
        let format = formats.get(format_index).or_else(|| formats.last());
        format_index += 1;
        let fields: Vec<_> = line.split(separator).collect();
        let row_index = rows.len();
        let mut row = Row {
            attr: Attr::default(),
            cells: Vec::new(),
        };
        for (column, specification) in columns.iter().enumerate() {
            let field = fields.get(column).copied().unwrap_or_default();
            let descriptor = format.and_then(|format| format.get(column));
            if matches!(descriptor, Some(ColumnFormat::HorizontalSpan))
                && let Some(anchor) = column
                    .checked_sub(1)
                    .and_then(|left| anchors.get(left))
                    .copied()
                    .flatten()
            {
                if anchor.0 == row_index
                    && let Some(cell) = row.cells.get_mut(anchor.1)
                {
                    cell.col_span = cell.col_span.saturating_add(1);
                }
                if let Some(slot) = anchors.get_mut(column) {
                    *slot = Some(anchor);
                }
                continue;
            }
            if (matches!(descriptor, Some(ColumnFormat::VerticalSpan)) || field.trim() == "\\^")
                && let Some((above_row, above_cell)) = anchors.get(column).copied().flatten()
                && let Some(cell) = rows
                    .get_mut(above_row)
                    .and_then(|row| row.cells.get_mut(above_cell))
            {
                cell.row_span = i64::try_from(row_index.saturating_sub(above_row))
                    .unwrap_or(i64::MAX)
                    .saturating_add(1);
                continue;
            }
            let mut cell = tbl_cell(field);
            if let Some(ColumnFormat::Aligned(alignment)) = descriptor
                && *alignment != specification.align
            {
                cell.align = alignment.clone();
            }
            if let Some(slot) = anchors.get_mut(column) {
                *slot = Some((row_index, row.cells.len()));
            }
            row.cells.push(cell);
        }
        rows.push(row);
    }
    (rows, header)
}

/// Reads the cell separator from a tbl options line's `tab(X)` option, if it carries one.
fn tab_option(options: &str) -> Option<String> {
    let inside = options.split_once("tab(")?.1.split_once(')')?.0;
    (!inside.is_empty()).then(|| inside.to_owned())
}

/// Parses column classifiers and spans, skipping modifier arguments.
fn parse_column_formats(specification: &str) -> Vec<Vec<ColumnFormat>> {
    let mut formats = Vec::new();
    let mut columns = Vec::new();
    let mut characters = specification.chars().peekable();
    while let Some(character) = characters.next() {
        match character.to_ascii_lowercase() {
            'l' | 'a' => columns.push(ColumnFormat::Aligned(Alignment::AlignLeft)),
            'r' | 'n' => columns.push(ColumnFormat::Aligned(Alignment::AlignRight)),
            'c' => columns.push(ColumnFormat::Aligned(Alignment::AlignCenter)),
            's' => columns.push(ColumnFormat::HorizontalSpan),
            '^' => columns.push(ColumnFormat::VerticalSpan),
            ',' => {
                if !columns.is_empty() {
                    formats.push(std::mem::take(&mut columns));
                }
            }
            'f' | 'm' => match characters.peek() {
                Some('(') => {
                    characters.next();
                    read_delimited(&mut characters, ')');
                }
                Some('[') => {
                    characters.next();
                    read_delimited(&mut characters, ']');
                }
                _ => {
                    for _ in 0..2 {
                        if characters.peek().is_some_and(char::is_ascii_alphanumeric) {
                            characters.next();
                        } else {
                            break;
                        }
                    }
                }
            },
            'w' | 'p' | 'v' => {
                if characters.peek() == Some(&'(') {
                    characters.next();
                    read_delimited(&mut characters, ')');
                } else {
                    while matches!(characters.peek(), Some(character) if character.is_ascii_digit())
                    {
                        characters.next();
                    }
                }
            }
            _ => {}
        }
    }
    if !columns.is_empty() {
        formats.push(columns);
    }
    formats
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
