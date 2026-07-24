//! Org tables: row collection, header/body splitting, and cell parsing.

use std::collections::BTreeMap;
use std::mem;

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Row, Table, TableBody, TableFoot,
    TableHead,
};
use carta_core::Extensions;

use super::blocks::Affiliated;
use super::inline::parse_inlines;

pub(super) fn is_table_line(line: &str) -> bool {
    line.trim_start().starts_with('|')
}

/// One parsed table row: either a separator (`|---+---|`) or content cells.
pub(super) enum TableRow {
    Separator,
    Cells(Vec<String>),
}

pub(super) fn collect_table(lines: &[&str], start: usize) -> (Vec<TableRow>, usize) {
    let mut rows = Vec::new();
    let mut i = start;
    while let Some(&line) = lines.get(i) {
        if !is_table_line(line) {
            break;
        }
        rows.push(parse_table_row(line));
        i += 1;
    }
    (rows, i - start)
}

fn parse_table_row(line: &str) -> TableRow {
    let t = line.trim();
    let inner = t.strip_prefix('|').unwrap_or(t);
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    if !inner.is_empty()
        && inner
            .chars()
            .all(|c| matches!(c, '-' | '+' | '|' | ' ' | ':'))
    {
        return TableRow::Separator;
    }
    let cells = inner.split('|').map(|c| c.trim().to_owned()).collect();
    TableRow::Cells(cells)
}

pub(super) fn build_table(
    rows: &[TableRow],
    ext: Extensions,
    notes: &BTreeMap<String, Vec<Block>>,
    pending: &mut Affiliated,
) -> Block {
    let mut head_rows: Vec<Vec<String>> = Vec::new();
    let mut body_rows: Vec<Vec<String>> = Vec::new();
    let mut seen_separator = false;
    let mut header_done = false;
    for row in rows {
        match row {
            TableRow::Separator => {
                if !body_rows.is_empty() {
                    header_done = true;
                } else if !head_rows.is_empty() {
                    seen_separator = true;
                }
            }
            TableRow::Cells(cells) => {
                if seen_separator || header_done {
                    body_rows.push(cells.clone());
                } else {
                    head_rows.push(cells.clone());
                }
            }
        }
    }
    if !seen_separator {
        body_rows.splice(0..0, head_rows.drain(..));
    }

    let columns = head_rows
        .iter()
        .chain(body_rows.iter())
        .map(Vec::len)
        .max()
        .unwrap_or(0);

    let col_specs = (0..columns)
        .map(|_| ColSpec {
            align: Alignment::AlignDefault,
            width: ColWidth::ColWidthDefault,
        })
        .collect();

    let to_rows = |cells: &[Vec<String>]| -> Vec<Row> {
        cells
            .iter()
            .map(|row| Row {
                attr: Attr::default(),
                cells: (0..columns)
                    .map(|c| build_cell(row.get(c).map_or("", String::as_str), ext, notes))
                    .collect(),
            })
            .collect()
    };

    let Affiliated { caption, name } = mem::take(pending);
    let caption = Caption {
        short: None,
        long: caption.map(|c| vec![Block::Plain(c)]).unwrap_or_default(),
    };

    let table = Table {
        attr: Attr {
            id: name.unwrap_or_default().into(),
            ..Attr::default()
        },
        caption,
        col_specs,
        head: TableHead {
            attr: Attr::default(),
            rows: to_rows(&head_rows),
        },
        bodies: vec![TableBody {
            attr: Attr::default(),
            row_head_columns: 0,
            head: Vec::new(),
            body: to_rows(&body_rows),
        }],
        foot: TableFoot::default(),
    };
    Block::Table(Box::new(table))
}

fn build_cell(text: &str, ext: Extensions, notes: &BTreeMap<String, Vec<Block>>) -> Cell {
    let content = if text.is_empty() {
        Vec::new()
    } else {
        vec![Block::Plain(parse_inlines(text, ext, notes))]
    };
    Cell {
        attr: Attr::default(),
        align: Alignment::AlignDefault,
        row_span: 1,
        col_span: 1,
        content,
    }
}
