//! Parsing of wikitext tables into the table model.

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Row, Table, TableBody, TableFoot,
    TableHead,
};

use super::{Parser, at, collect_range, line_end};

/// A table cell collected during the line scan, before its text is parsed into blocks. A `!`-marked
/// cell is a header cell; the spans and attributes come from the cell's leading attribute list.
struct RawCell {
    is_header: bool,
    align: Alignment,
    col_span: i32,
    row_span: i32,
    attr: Attr,
    content: String,
}

/// The alignment, spans, and attributes parsed from a cell's leading attribute list.
struct CellAttrs {
    align: Alignment,
    col_span: i32,
    row_span: i32,
    attr: Attr,
}

/// Which open construct a table continuation line extends.
#[derive(Clone, Copy)]
enum OpenTarget {
    None,
    Caption,
    Cell,
}

impl Parser {
    /// Parses a `{|`-delimited table into a [`Block::Table`], returning the index past the closing
    /// `|}`. Table and row attribute lists are dropped; a cell's attribute list supplies its
    /// alignment, spans, identifier, and classes. The first row becomes the header when its first
    /// cell is a `!` header cell.
    pub(super) fn parse_table(&mut self, chars: &[char], pos: usize) -> (Block, usize) {
        let after = table_block_end(chars, pos);
        let region = collect_range(chars, pos, after);
        (self.build_table(&region), after)
    }

    fn build_table(&mut self, region: &str) -> Block {
        let (mut rows, caption_text) = scan_table_region(region);
        // The first row may omit its leading `|-`, so one seen before any cell opens the first row
        // rather than closing an empty one; every later `|-` closes a row, keeping empty rows.
        if rows.first().is_some_and(Vec::is_empty) {
            rows.remove(0);
        }
        if rows.is_empty() {
            // A table with no cells still yields one empty row.
            rows.push(Vec::new());
        }

        let n_rows = rows.len();
        // The first row fixes the column count; cells that overflow it in later rows are dropped.
        let ncols = rows.first().map_or(0, |r| {
            r.iter().map(|c| col_count(c.col_span)).sum::<usize>()
        });
        let col_specs = column_specs(&rows, ncols);

        let is_header_first = rows
            .first()
            .and_then(|r| r.first())
            .is_some_and(|c| c.is_header);

        let ast_rows = self.lay_grid(&rows, ncols, n_rows);

        let (head_rows, body_rows) = if is_header_first {
            let mut iter = ast_rows.into_iter();
            let head: Vec<Row> = iter.next().into_iter().collect();
            (head, iter.collect::<Vec<Row>>())
        } else {
            (Vec::new(), ast_rows)
        };

        let caption = match caption_text {
            Some(text) => {
                let inlines = self.parse_inlines(text.trim());
                if inlines.is_empty() {
                    Caption::default()
                } else {
                    Caption {
                        short: None,
                        long: vec![Block::Plain(inlines)],
                    }
                }
            }
            None => Caption::default(),
        };

        Block::Table(Box::new(Table {
            attr: Attr::default(),
            caption,
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

    /// Lays the parsed cells onto a fixed `ncols`-wide grid so spans stay in bounds: a `rowspan`
    /// cannot reach past the last row, a `colspan` cannot reach past the last column (an overflowing
    /// cell is dropped), a cell skips columns still covered by a `rowspan` from an earlier row, and
    /// any column a row leaves uncovered is filled with an empty cell.
    fn lay_grid(&mut self, rows: &[Vec<RawCell>], ncols: usize, n_rows: usize) -> Vec<Row> {
        let mut ast_rows: Vec<Row> = Vec::new();
        let mut occupied: Vec<i32> = vec![0; ncols];
        for (r, raw) in rows.iter().enumerate() {
            let available = i32::try_from(n_rows.saturating_sub(r)).unwrap_or(i32::MAX);
            let mut cells: Vec<Cell> = Vec::new();
            let mut col = 0usize;
            for c in raw {
                while col < ncols && occupied.get(col).copied().unwrap_or(0) > 0 {
                    col += 1;
                }
                if col >= ncols {
                    break;
                }
                let col_span = col_count(c.col_span).min(ncols - col);
                let row_span = c.row_span.max(1).min(available);
                let content_chars: Vec<char> = c.content.trim().chars().collect();
                let content = self.parse_cell_blocks(&content_chars);
                cells.push(Cell {
                    attr: c.attr.clone(),
                    align: c.align.clone(),
                    row_span,
                    col_span: i32::try_from(col_span).unwrap_or(1),
                    content,
                });
                for k in col..col + col_span {
                    if let Some(slot) = occupied.get_mut(k) {
                        *slot = row_span;
                    }
                }
                col += col_span;
            }
            while col < ncols {
                if occupied.get(col).copied().unwrap_or(0) == 0 {
                    cells.push(empty_cell());
                }
                col += 1;
            }
            for slot in &mut occupied {
                *slot = (*slot - 1).max(0);
            }
            ast_rows.push(Row {
                attr: Attr::default(),
                cells,
            });
        }
        ast_rows
    }
}

/// Finds the index one past the end of a table block opening with `{|` at `pos`. Opening (`{|`) and
/// closing (`|}`) markers are matched by depth, scanning whole lines, so a nested table does not
/// close the outer one early; an unterminated table runs to the end of input.
fn table_block_end(chars: &[char], pos: usize) -> usize {
    let n = chars.len();
    let mut depth = 0usize;
    let mut line = pos;
    loop {
        let mut content = line;
        while matches!(at(chars, content), Some(' ' | '\t')) {
            content += 1;
        }
        if at(chars, content) == Some('{') && at(chars, content + 1) == Some('|') {
            depth += 1;
        } else if at(chars, content) == Some('|') && at(chars, content + 1) == Some('}') {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return content + 2;
            }
        }
        let le = line_end(chars, line);
        if le >= n {
            return n;
        }
        line = le + 1;
    }
}

/// Scans the body of a `{|…|}` region into its rows of raw cells and an optional caption.
/// Each `|-` closes the current row; nested tables are passed through verbatim as cell content.
fn scan_table_region(region: &str) -> (Vec<Vec<RawCell>>, Option<String>) {
    let mut caption_text: Option<String> = None;
    let mut rows: Vec<Vec<RawCell>> = Vec::new();
    let mut cur: Vec<RawCell> = Vec::new();
    let mut open = OpenTarget::None;
    let mut nest = 0i32;

    let mut lines = region.lines();
    lines.next(); // The opening `{|` line; any table attribute list it carries is dropped.
    for line in lines {
        let trimmed = line.trim_start();
        if nest > 0 {
            if trimmed.starts_with("{|") {
                nest += 1;
            } else if trimmed.starts_with("|}") {
                nest -= 1;
            }
            append_continuation(open, &mut cur, &mut caption_text, line);
            continue;
        }
        if trimmed.starts_with("|}") {
            break;
        }
        if trimmed.starts_with("{|") {
            nest += 1;
            append_continuation(open, &mut cur, &mut caption_text, line);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("|+") {
            caption_text = Some(rest.to_string());
            open = OpenTarget::Caption;
            continue;
        }
        if trimmed.starts_with("|-") {
            rows.push(std::mem::take(&mut cur));
            open = OpenTarget::None;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('|') {
            cur.extend(parse_cell_line(false, rest));
            open = OpenTarget::Cell;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('!') {
            cur.extend(parse_cell_line(true, rest));
            open = OpenTarget::Cell;
            continue;
        }
        append_continuation(open, &mut cur, &mut caption_text, line);
    }
    rows.push(cur);
    (rows, caption_text)
}

/// Builds the column specifications from the first row, taking each column's alignment from the
/// cell that opens it and defaulting every column's width.
fn column_specs(rows: &[Vec<RawCell>], ncols: usize) -> Vec<ColSpec> {
    let mut aligns: Vec<Alignment> = Vec::new();
    if let Some(first) = rows.first() {
        for cell in first {
            for _ in 0..col_count(cell.col_span) {
                aligns.push(cell.align.clone());
            }
        }
    }
    aligns.resize(ncols, Alignment::AlignDefault);
    aligns
        .into_iter()
        .map(|align| ColSpec {
            align,
            width: ColWidth::ColWidthDefault,
        })
        .collect()
}

/// The number of grid columns a cell spans, never less than one.
fn col_count(col_span: i32) -> usize {
    usize::try_from(col_span.max(1)).unwrap_or(1)
}

/// A blank single-column cell used to fill a row that covers fewer columns than the table is wide.
fn empty_cell() -> Cell {
    Cell {
        attr: Attr::default(),
        align: Alignment::AlignDefault,
        row_span: 1,
        col_span: 1,
        content: Vec::new(),
    }
}

fn append_continuation(
    open: OpenTarget,
    cur: &mut [RawCell],
    caption: &mut Option<String>,
    line: &str,
) {
    match open {
        OpenTarget::Cell => {
            if let Some(cell) = cur.last_mut() {
                cell.content.push('\n');
                cell.content.push_str(line);
            }
        }
        OpenTarget::Caption => {
            if let Some(text) = caption {
                text.push('\n');
                text.push_str(line);
            }
        }
        OpenTarget::None => {}
    }
}

/// Splits one cell-marker line into its cells. A `|` data line separates cells with `||`; a `!`
/// header line additionally separates them with `!!`.
fn parse_cell_line(is_header: bool, rest: &str) -> Vec<RawCell> {
    split_cells(rest, is_header)
        .iter()
        .map(|chunk| parse_cell_chunk(is_header, chunk))
        .collect()
}

/// Splits a marker line's text into per-cell chunks at top-level `||` (and, for a header line, `!!`)
/// separators, leaving separators inside `[…]` or `{…}` groups untouched.
fn split_cells(s: &str, header: bool) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut out: Vec<String> = Vec::new();
    let mut start = 0usize;
    let mut square = 0i32;
    let mut curly = 0i32;
    let mut i = 0usize;
    while i < n {
        match at(&chars, i) {
            Some('[') => square += 1,
            Some(']') => square = (square - 1).max(0),
            Some('{') => curly += 1,
            Some('}') => curly = (curly - 1).max(0),
            _ => {}
        }
        if square == 0 && curly == 0 {
            let pipe = at(&chars, i) == Some('|') && at(&chars, i + 1) == Some('|');
            let bang = header && at(&chars, i) == Some('!') && at(&chars, i + 1) == Some('!');
            if pipe || bang {
                out.push(collect_range(&chars, start, i));
                i += 2;
                start = i;
                continue;
            }
        }
        i += 1;
    }
    out.push(collect_range(&chars, start, n));
    out
}

/// Parses one cell chunk into a [`RawCell`], splitting a leading attribute list from the content at
/// the first top-level `|` when the text before it is a valid attribute list.
fn parse_cell_chunk(is_header: bool, chunk: &str) -> RawCell {
    if let Some(idx) = find_attr_pipe(chunk)
        && let Some(attrs) = parse_cell_attrs(chunk.get(..idx).unwrap_or(""))
    {
        return RawCell {
            is_header,
            align: attrs.align,
            col_span: attrs.col_span,
            row_span: attrs.row_span,
            attr: attrs.attr,
            content: chunk.get(idx + 1..).unwrap_or("").to_string(),
        };
    }
    RawCell {
        is_header,
        align: Alignment::AlignDefault,
        col_span: 1,
        row_span: 1,
        attr: Attr::default(),
        content: chunk.to_string(),
    }
}

/// Finds the byte offset of the first top-level `|` in a cell chunk (the boundary between a leading
/// attribute list and the cell content), skipping any `|` inside `[…]` or `{…}` groups.
fn find_attr_pipe(s: &str) -> Option<usize> {
    let mut square = 0i32;
    let mut curly = 0i32;
    let mut in_quote = false;
    for (i, ch) in s.char_indices() {
        if in_quote {
            if ch == '"' {
                in_quote = false;
            }
            continue;
        }
        match ch {
            '"' => in_quote = true,
            '[' => square += 1,
            ']' => square = (square - 1).max(0),
            '{' => curly += 1,
            '}' => curly = (curly - 1).max(0),
            '|' if square == 0 && curly == 0 => return Some(i),
            _ => {}
        }
    }
    None
}

/// Parses a cell's leading attribute list. `align` maps to a column alignment, `colspan`/`rowspan`
/// to spans, `id`/`class` to the cell's identifier and classes, and everything else to a key/value
/// attribute. A bare token without a value is not a valid attribute list, so the whole text is
/// content instead (signalled by [`None`]).
fn parse_cell_attrs(s: &str) -> Option<CellAttrs> {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut i = 0usize;
    let mut id = String::new();
    let mut classes: Vec<String> = Vec::new();
    let mut attributes: Vec<(String, String)> = Vec::new();
    let mut align = Alignment::AlignDefault;
    let mut col_span = 1i32;
    let mut row_span = 1i32;
    let mut any = false;
    while i < n {
        while at(&chars, i).is_some_and(char::is_whitespace) {
            i += 1;
        }
        if i >= n {
            break;
        }
        let name_start = i;
        while at(&chars, i).is_some_and(|c| !c.is_whitespace() && c != '=') {
            i += 1;
        }
        let name = collect_range(&chars, name_start, i);
        if name.is_empty() || at(&chars, i) != Some('=') {
            return None;
        }
        i += 1;
        let value = if at(&chars, i) == Some('"') {
            i += 1;
            let value_start = i;
            while at(&chars, i).is_some_and(|c| c != '"') {
                i += 1;
            }
            let value = collect_range(&chars, value_start, i);
            if at(&chars, i) == Some('"') {
                i += 1;
            }
            value
        } else {
            let value_start = i;
            while at(&chars, i).is_some_and(|c| !c.is_whitespace()) {
                i += 1;
            }
            collect_range(&chars, value_start, i)
        };
        any = true;
        match name.to_ascii_lowercase().as_str() {
            "id" => id = value,
            "class" => classes.extend(value.split_whitespace().map(str::to_string)),
            "align" => match value.to_ascii_lowercase().as_str() {
                "left" => align = Alignment::AlignLeft,
                "right" => align = Alignment::AlignRight,
                "center" => align = Alignment::AlignCenter,
                _ => attributes.push(("align".to_string(), value)),
            },
            // One grid slot per spanned column: clamp to the HTML spec's span limits, or an attacker-supplied span forces a huge allocation.
            "colspan" => match value.trim().parse::<i32>() {
                Ok(v) if v >= 1 => col_span = v.min(1000),
                _ => attributes.push(("colspan".to_string(), value)),
            },
            "rowspan" => match value.trim().parse::<i32>() {
                Ok(v) if v >= 1 => row_span = v.min(65534),
                _ => attributes.push(("rowspan".to_string(), value)),
            },
            _ => attributes.push((name, value)),
        }
    }
    if !any {
        return None;
    }
    Some(CellAttrs {
        align,
        col_span,
        row_span,
        attr: Attr {
            id: id.into(),
            classes: classes.into_iter().map(Into::into).collect(),
            attributes: attributes
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        },
    })
}
