//! Grid, simple, CSV and list table construction.

use super::directives::{cells_to_rows, directive_count, directive_widths, list_row, parse_csv};
use super::{Parser, dedent, indent_of, is_blank, line_at};
use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Row, Table, TableBody, TableFoot,
    TableHead,
};
use std::collections::VecDeque;

impl Parser<'_> {
    // --- table directives ---

    /// A `csv-table` directive: its rows are comma-separated values, with an optional explicit
    /// `:header:` row and/or a count of leading `:header-rows:` taken from the data.
    pub(super) fn csv_table(
        &mut self,
        argument: &str,
        options: &[(String, String)],
        content: &[String],
        out: &mut Vec<Block>,
    ) {
        let widths = directive_widths(options);
        let mut records = parse_csv(&content.join("\n"));
        let mut header_records: Vec<Vec<String>> = Vec::new();
        if let Some((_, header)) = options.iter().find(|(k, _)| k == "header") {
            header_records.extend(parse_csv(header));
        }
        let take = directive_count(options, "header-rows").min(records.len());
        header_records.extend(records.drain(..take));
        let num_cols = header_records
            .iter()
            .chain(records.iter())
            .map(Vec::len)
            .max()
            .unwrap_or(0);
        if num_cols == 0 {
            return;
        }
        let head_rows = header_records
            .iter()
            .map(|r| self.csv_row(r, num_cols))
            .collect();
        let body_rows = records.iter().map(|r| self.csv_row(r, num_cols)).collect();
        out.push(self.make_table(argument, widths.as_deref(), head_rows, body_rows, num_cols));
    }

    fn csv_row(&mut self, fields: &[String], num_cols: usize) -> Vec<Cell> {
        (0..num_cols)
            .map(|i| {
                let content = match fields.get(i) {
                    Some(f) if !f.is_empty() => vec![Block::Plain(self.inlines(f))],
                    _ => Vec::new(),
                };
                Cell {
                    attr: Attr::default(),
                    align: Alignment::AlignDefault,
                    row_span: 1,
                    col_span: 1,
                    content,
                }
            })
            .collect()
    }

    /// A `list-table` directive: a two-level bullet list where each outer item is a row and its
    /// nested bullet list supplies the row's cells.
    pub(super) fn list_table(
        &mut self,
        argument: &str,
        options: &[(String, String)],
        content: &[String],
        out: &mut Vec<Block>,
    ) {
        let widths = directive_widths(options);
        let mut rows: Vec<Vec<Vec<Block>>> = Vec::new();
        for block in self.blocks(content) {
            if let Block::BulletList(items) = block {
                for item in items {
                    let mut cells = Vec::new();
                    for inner in item {
                        if let Block::BulletList(cell_items) = inner {
                            cells.extend(cell_items);
                        }
                    }
                    rows.push(cells);
                }
            }
        }
        let num_cols = rows.iter().map(Vec::len).max().unwrap_or(0);
        if num_cols == 0 {
            return;
        }
        let take = directive_count(options, "header-rows").min(rows.len());
        let head_src: Vec<Vec<Vec<Block>>> = rows.drain(..take).collect();
        let head_rows = head_src
            .into_iter()
            .map(|r| list_row(r, num_cols))
            .collect();
        let body_rows = rows.into_iter().map(|r| list_row(r, num_cols)).collect();
        out.push(self.make_table(argument, widths.as_deref(), head_rows, body_rows, num_cols));
    }

    /// Assemble a table from already-built header and body cell rows, a caption drawn from the
    /// directive argument, and either explicit column widths or the default.
    fn make_table(
        &mut self,
        caption: &str,
        widths: Option<&[f64]>,
        head_rows: Vec<Vec<Cell>>,
        body_rows: Vec<Vec<Cell>>,
        num_cols: usize,
    ) -> Block {
        let caption = if caption.trim().is_empty() {
            Caption::default()
        } else {
            Caption {
                short: None,
                long: vec![Block::Plain(self.inlines(caption.trim()))],
            }
        };
        let col_specs = (0..num_cols)
            .map(|i| ColSpec {
                align: Alignment::AlignDefault,
                width: match widths {
                    Some(w) if w.len() == num_cols => w
                        .get(i)
                        .copied()
                        .map_or(ColWidth::ColWidthDefault, ColWidth::ColWidth),
                    _ => ColWidth::ColWidthDefault,
                },
            })
            .collect();
        Block::Table(Box::new(Table {
            attr: Attr::default(),
            caption,
            col_specs,
            head: TableHead {
                attr: Attr::default(),
                rows: cells_to_rows(head_rows),
            },
            bodies: vec![TableBody {
                attr: Attr::default(),
                row_head_columns: 0,
                head: Vec::new(),
                body: cells_to_rows(body_rows),
            }],
            foot: TableFoot::default(),
        }))
    }

    // --- grid tables ---

    // Column widths are small character spans, far inside f64's exact-integer range.
    #[allow(clippy::cast_precision_loss)]
    // One-pass walk over the character matrix; splitting would scatter shared cursor state.
    #[allow(clippy::too_many_lines)]
    pub(super) fn grid_table(
        &mut self,
        lines: &[String],
        start: usize,
        out: &mut Vec<Block>,
    ) -> Option<usize> {
        // The table runs over consecutive lines that belong to the grid (a border or a `|`-led row).
        let mut end = start;
        while lines.get(end).is_some_and(|l| is_grid_line(l)) {
            end += 1;
        }
        if end - start < 3 {
            return None;
        }
        // A padded character matrix so every position can be addressed by (row, column).
        let width = (start..end)
            .filter_map(|i| lines.get(i))
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(0);
        let block: Vec<Vec<char>> = (start..end)
            .filter_map(|i| lines.get(i))
            .map(|l| {
                let mut row: Vec<char> = l.chars().collect();
                row.resize(width, ' ');
                row
            })
            .collect();

        let cells = scan_grid_cells(&block)?;
        if cells.is_empty() {
            return None;
        }

        // The vertical and horizontal grid lines, as the distinct cell-edge positions.
        let mut col_edges: Vec<usize> = cells.iter().flat_map(|c| [c.left, c.right]).collect();
        col_edges.sort_unstable();
        col_edges.dedup();
        let mut row_edges: Vec<usize> = cells.iter().flat_map(|c| [c.top, c.bottom]).collect();
        row_edges.sort_unstable();
        row_edges.dedup();
        let col_index = |pos: usize| col_edges.iter().position(|e| *e == pos);
        let row_index = |pos: usize| row_edges.iter().position(|e| *e == pos);
        let num_cols = col_edges.len().checked_sub(1)?;
        let num_rows = row_edges.len().checked_sub(1)?;
        if num_cols == 0 || num_rows == 0 {
            return None;
        }

        // Place each cell into a row/column grid, validating that the cells tile it exactly.
        let mut grid: Vec<Vec<Option<GridCell>>> = vec![vec![None; num_cols]; num_rows];
        let mut covered = vec![vec![false; num_cols]; num_rows];
        for cell in &cells {
            let r0 = row_index(cell.top)?;
            let r1 = row_index(cell.bottom)?;
            let c0 = col_index(cell.left)?;
            let c1 = col_index(cell.right)?;
            let text: String = (cell.top + 1..cell.bottom)
                .filter_map(|r| block.get(r))
                .map(|row| {
                    let seg: String = row
                        .get(cell.left + 1..cell.right)
                        .map_or_else(String::new, |s| s.iter().collect());
                    seg.trim_end().to_string()
                })
                .collect::<Vec<_>>()
                .join("\n");
            for r in r0..r1 {
                for c in c0..c1 {
                    if covered.get(r).and_then(|row| row.get(c)).copied() != Some(false) {
                        return None;
                    }
                    if let Some(slot) = covered.get_mut(r).and_then(|row| row.get_mut(c)) {
                        *slot = true;
                    }
                }
            }
            if let Some(slot) = grid.get_mut(r0).and_then(|row| row.get_mut(c0)) {
                *slot = Some(GridCell {
                    text,
                    row_span: r1 - r0,
                    col_span: c1 - c0,
                });
            }
        }
        if covered.iter().any(|row| row.iter().any(|c| !c)) {
            return None;
        }

        // A `=` separator line marks the boundary between header rows and body rows.
        let header_rows = row_edges
            .iter()
            .position(|edge| block.get(*edge).is_some_and(|row| row.contains(&'=')))
            .unwrap_or(0);

        let last = *col_edges.last()?;
        let first = *col_edges.first()?;
        let total = last.saturating_sub(first).saturating_sub(num_cols);
        let divisor = total.max(72) as f64;
        let col_specs: Vec<ColSpec> = (0..num_cols)
            .map(|i| {
                let lo = col_edges.get(i).copied().unwrap_or(0);
                let hi = col_edges.get(i + 1).copied().unwrap_or(lo);
                ColSpec {
                    align: Alignment::AlignDefault,
                    width: ColWidth::ColWidth(hi.saturating_sub(lo) as f64 / divisor),
                }
            })
            .collect();

        let mut head_rows = Vec::new();
        let mut body_rows = Vec::new();
        for (r, row) in grid.iter().enumerate() {
            let built = self.grid_row(row);
            if r < header_rows {
                head_rows.push(built);
            } else {
                body_rows.push(built);
            }
        }

        let table = Table {
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
        };
        out.push(Block::Table(Box::new(table)));
        Some(end)
    }

    /// Build one table row, emitting only the cells that originate in this row band; positions
    /// covered by a row- or column-spanning cell that began earlier carry no cell of their own.
    fn grid_row(&mut self, row: &[Option<GridCell>]) -> Row {
        let cells = row
            .iter()
            .filter_map(|slot| slot.as_ref())
            .map(|cell| {
                let row_span = i32::try_from(cell.row_span).unwrap_or(1);
                let col_span = i32::try_from(cell.col_span).unwrap_or(1);
                self.text_cell(&cell.text, row_span, col_span)
            })
            .collect();
        Row {
            attr: Attr::default(),
            cells,
        }
    }

    /// Build a cell from its newline-joined text. The shared blank-edges/min-indent normalization is
    /// applied, the text is parsed as block content, and a lone paragraph is demoted to a plain block.
    fn text_cell(&mut self, text: &str, row_span: i32, col_span: i32) -> Cell {
        let raw: Vec<String> = text.split('\n').map(str::to_string).collect();
        let trimmed = trim_blank_edges(raw);
        let min_indent = trimmed
            .iter()
            .filter(|l| !is_blank(l))
            .map(|l| indent_of(l))
            .min()
            .unwrap_or(0);
        let region: Vec<String> = trimmed
            .iter()
            .map(|l| {
                if is_blank(l) {
                    String::new()
                } else {
                    dedent(l, min_indent)
                }
            })
            .collect();
        let mut content = self.blocks(&region);
        if let [Block::Para(_)] = content.as_slice()
            && let Some(Block::Para(inlines)) = content.pop()
        {
            content.push(Block::Plain(inlines));
        }
        Cell {
            attr: Attr::default(),
            align: Alignment::AlignDefault,
            row_span,
            col_span,
            content,
        }
    }

    // --- simple tables ---

    /// Parse a simple table beginning at its top border. Columns come from the `=` runs of the top
    /// border; the bottom border is the first `=` border followed by a blank line or the end of
    /// input, and any earlier interior `=` border separates the header rows from the body. Returns
    /// `None` (so the caller falls back to paragraph parsing) when no bottom border is found.
    pub(super) fn simple_table(
        &mut self,
        lines: &[String],
        start: usize,
        out: &mut Vec<Block>,
    ) -> Option<usize> {
        let columns = simple_columns(line_at(lines, start))?;
        let mut header_end: Option<usize> = None;
        let mut bottom: Option<usize> = None;
        let mut i = start + 1;
        while let Some(line) = lines.get(i) {
            if is_equals_border(line) {
                let next_blank = lines.get(i + 1).is_none_or(|l| is_blank(l));
                if next_blank {
                    bottom = Some(i);
                    break;
                }
                if header_end.is_none() {
                    header_end = Some(i);
                }
            }
            i += 1;
        }
        let bottom = bottom?;
        let header_lines: Vec<String> = match header_end {
            Some(end) => (start + 1..end)
                .filter_map(|j| lines.get(j).cloned())
                .collect(),
            None => Vec::new(),
        };
        let body_start = header_end.map_or(start + 1, |end| end + 1);
        let body_lines: Vec<String> = (body_start..bottom)
            .filter_map(|j| lines.get(j).cloned())
            .collect();

        let head_rows = self.simple_rows(&header_lines, &columns);
        let body_rows = self.simple_rows(&body_lines, &columns);

        let col_specs: Vec<ColSpec> = columns
            .iter()
            .map(|_| ColSpec {
                align: Alignment::AlignDefault,
                width: ColWidth::ColWidthDefault,
            })
            .collect();
        let table = Table {
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
        };
        out.push(Block::Table(Box::new(table)));
        Some(bottom + 1)
    }

    /// Group a region's lines into table rows. A line whose first column carries text starts a new
    /// row; a text line with a blank first column continues the current one. A `-` underline ends the
    /// row above it, joining the columns its filled margins span.
    fn simple_rows(&mut self, lines: &[String], columns: &[(usize, usize)]) -> Vec<Row> {
        let mut rows = Vec::new();
        let mut current: Vec<String> = Vec::new();
        for line in lines {
            if let Some(groups) = span_underline_groups(line, columns) {
                if !current.is_empty() {
                    rows.push(self.simple_row(&current, columns, &groups));
                    current.clear();
                }
                continue;
            }
            if is_blank(line) {
                if !current.is_empty() {
                    current.push(String::new());
                }
                continue;
            }
            if !current.is_empty() && first_column_blank(line, columns) {
                current.push(line.clone());
            } else {
                if !current.is_empty() {
                    let groups = default_groups(columns.len());
                    rows.push(self.simple_row(&current, columns, &groups));
                    current.clear();
                }
                current.push(line.clone());
            }
        }
        if !current.is_empty() {
            let groups = default_groups(columns.len());
            rows.push(self.simple_row(&current, columns, &groups));
        }
        rows
    }

    fn simple_row(
        &mut self,
        row_lines: &[String],
        columns: &[(usize, usize)],
        groups: &[(usize, usize)],
    ) -> Row {
        let last_col = columns.len().saturating_sub(1);
        let cells = groups
            .iter()
            .map(|(a, b)| {
                let lo = columns.get(*a).map_or(0, |c| c.0);
                let hi = if *b >= last_col {
                    usize::MAX
                } else {
                    columns.get(b + 1).map_or(usize::MAX, |c| c.0)
                };
                let text = row_lines
                    .iter()
                    .map(|line| {
                        let cs: Vec<char> = line.chars().collect();
                        let end = hi.min(cs.len());
                        let seg: String = cs
                            .get(lo..end)
                            .map(|s| s.iter().collect())
                            .unwrap_or_default();
                        seg.trim_end().to_string()
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                self.text_cell(&text, 1, i32::try_from(b - a + 1).unwrap_or(1))
            })
            .collect();
        Row {
            attr: Attr::default(),
            cells,
        }
    }
}

// --- grid table helpers ------------------------------------------------------------------------

fn is_grid_line(line: &str) -> bool {
    line.starts_with('+') || line.starts_with('|')
}

/// A cell rectangle traced out of a grid table, in (line, column) matrix coordinates: its corners
/// are the `+` at the top-left and the `+` at the bottom-right.
struct ScanCell {
    top: usize,
    left: usize,
    bottom: usize,
    right: usize,
}

/// A placed grid-table cell: its raw interior text and its extent in row and column bands.
#[derive(Clone)]
struct GridCell {
    text: String,
    row_span: usize,
    col_span: usize,
}

fn grid_at(block: &[Vec<char>], row: usize, col: usize) -> Option<char> {
    block.get(row).and_then(|r| r.get(col)).copied()
}

/// Trace every cell of a grid table out of its character matrix. From the top-left corner, each
/// cell rectangle is found by following its top edge right to a `+`, its right edge down to a `+`,
/// its bottom edge left to the starting column, and its left edge back up to the top, each edge
/// made solely of its border character (`-` across, `|` down), with `+` permitted where another
/// grid line crosses. The corners opposite each cell seed the search for its right and lower
/// neighbours. Returns `None` for a matrix that does not open with a corner.
fn scan_grid_cells(block: &[Vec<char>]) -> Option<Vec<ScanCell>> {
    let height = block.len();
    let width = block.first().map_or(0, Vec::len);
    if height < 2 || width < 2 || grid_at(block, 0, 0) != Some('+') {
        return None;
    }
    let bottom = height - 1;
    let right = width - 1;
    let mut cells = Vec::new();
    let mut visited = vec![vec![false; width]; height];
    let mut queue: VecDeque<(usize, usize)> = VecDeque::new();
    queue.push_back((0, 0));
    while let Some((top, left)) = queue.pop_front() {
        if top >= bottom || left >= right {
            continue;
        }
        if visited.get(top).and_then(|r| r.get(left)).copied() == Some(true) {
            continue;
        }
        if let Some(slot) = visited.get_mut(top).and_then(|r| r.get_mut(left)) {
            *slot = true;
        }
        let Some(cell) = trace_cell(block, top, left, bottom, right) else {
            continue;
        };
        queue.push_back((cell.top, cell.right));
        queue.push_back((cell.bottom, cell.left));
        cells.push(cell);
    }
    Some(cells)
}

fn trace_cell(
    block: &[Vec<char>],
    top: usize,
    left: usize,
    bottom: usize,
    right: usize,
) -> Option<ScanCell> {
    for col in left + 1..=right {
        match grid_at(block, top, col) {
            Some('+') => {
                if let Some(b) = scan_cell_down(block, top, left, col, bottom) {
                    return Some(ScanCell {
                        top,
                        left,
                        bottom: b,
                        right: col,
                    });
                }
            }
            // A `-` extends a body border; `=` extends the header/body separator.
            Some('-' | '=') => {}
            _ => return None,
        }
    }
    None
}

fn scan_cell_down(
    block: &[Vec<char>],
    top: usize,
    left: usize,
    right: usize,
    bottom: usize,
) -> Option<usize> {
    for row in top + 1..=bottom {
        match grid_at(block, row, right) {
            Some('+') => {
                if scan_cell_close(block, top, left, right, row) {
                    return Some(row);
                }
            }
            Some('|') => {}
            _ => return None,
        }
    }
    None
}

/// Verify the bottom and left edges of a candidate cell: the bottom edge from `right` back to
/// `left` is `-` (or a `+` crossing) and reaches a `+` at the bottom-left corner, and the left edge
/// from `bottom` back to `top` is `|` (or a `+` crossing).
fn scan_cell_close(
    block: &[Vec<char>],
    top: usize,
    left: usize,
    right: usize,
    bottom: usize,
) -> bool {
    for col in left + 1..right {
        if !matches!(grid_at(block, bottom, col), Some('-' | '=' | '+')) {
            return false;
        }
    }
    if grid_at(block, bottom, left) != Some('+') {
        return false;
    }
    for row in top + 1..bottom {
        if !matches!(grid_at(block, row, left), Some('|' | '+')) {
            return false;
        }
    }
    true
}

/// Whether a line is a simple-table ruler: two or more space-separated runs of `=`.
pub(super) fn is_simple_table_ruler(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty() && trimmed.starts_with('=') && trimmed.chars().all(|c| c == '=' || c == ' ')
}

/// The inclusive-exclusive character ranges of a simple table's columns, from the `=` runs of its
/// top border. `None` unless the border is made solely of `=` runs and spaces. A single column is
/// allowed: a lone `=` run is rejected as a section adornment or transition before the table parser
/// is reached, and the parser still requires a closing border to confirm a table.
fn simple_columns(border: &str) -> Option<Vec<(usize, usize)>> {
    let chars: Vec<char> = border.chars().collect();
    let mut columns = Vec::new();
    let mut i = 0;
    while let Some(c) = chars.get(i) {
        match c {
            '=' => {
                let start = i;
                while chars.get(i) == Some(&'=') {
                    i += 1;
                }
                columns.push((start, i));
            }
            ' ' => i += 1,
            _ => return None,
        }
    }
    (!columns.is_empty()).then_some(columns)
}

/// Whether a line is a `=` border: a non-empty run of `=` and spaces with no other content.
fn is_equals_border(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty() && trimmed.chars().all(|c| c == '=' || c == ' ')
}

/// Whether a line's first column holds no text, marking it a continuation of the row above.
fn first_column_blank(line: &str, columns: &[(usize, usize)]) -> bool {
    let chars: Vec<char> = line.chars().collect();
    let lo = columns.first().map_or(0, |c| c.0);
    let hi = columns.get(1).map_or(chars.len(), |c| c.0);
    (lo..hi).all(|p| chars.get(p).is_none_or(|c| c.is_whitespace()))
}

/// Each column standing alone, the column grouping a row carries when no span underline joins any.
fn default_groups(count: usize) -> Vec<(usize, usize)> {
    (0..count).map(|i| (i, i)).collect()
}

/// The column groups a `-` underline imposes on the row above it: a margin filled with `-` joins the
/// columns on either side into one span. `None` unless the line is solely `-` and spaces with at
/// least one `-`, which is what distinguishes an underline from cell text.
fn span_underline_groups(line: &str, columns: &[(usize, usize)]) -> Option<Vec<(usize, usize)>> {
    let chars: Vec<char> = line.chars().collect();
    let has_dash = chars.contains(&'-');
    if !has_dash || !chars.iter().all(|c| matches!(c, '-' | ' ')) {
        return None;
    }
    let mut groups = Vec::new();
    let mut group_start = 0;
    let n = columns.len();
    for i in 0..n.saturating_sub(1) {
        let left_end = columns.get(i).map_or(0, |c| c.1);
        let right_start = columns.get(i + 1).map_or(left_end, |c| c.0);
        let filled = (left_end..right_start).any(|p| chars.get(p) == Some(&'-'));
        if !filled {
            groups.push((group_start, i));
            group_start = i + 1;
        }
    }
    groups.push((group_start, n.saturating_sub(1)));
    Some(groups)
}

fn trim_blank_edges(mut lines: Vec<String>) -> Vec<String> {
    while lines.first().is_some_and(|l| is_blank(l)) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|l| is_blank(l)) {
        lines.pop();
    }
    lines
}
