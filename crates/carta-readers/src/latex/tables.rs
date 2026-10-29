//! Tabular parsing helpers: column specs, rows, rules, and option lists.

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Inline, Row, Table, TableBody,
    TableFoot, TableHead,
};

use super::support::trim_inlines;
use super::{InlineStop, Parser};

/// Parses a `key=value, key, …` option list into ordered attribute pairs. A bare key gets an empty
/// value.
pub(super) fn parse_key_values(text: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for part in split_top_level(text, ',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        match part.split_once('=') {
            Some((k, v)) => pairs.push((k.trim().to_owned(), v.trim().to_owned())),
            None => pairs.push((part.to_owned(), String::new())),
        }
    }
    pairs
}

/// Builds an image's attribute list from its bracketed options, keeping only the sizing keys and
/// expressing a fraction of the text block as a percentage.
pub(super) fn image_attributes(opts: &str) -> Vec<(String, String)> {
    let mut attrs = Vec::new();
    for (key, value) in parse_key_values(opts) {
        if key != "width" && key != "height" {
            continue;
        }
        let value = latex_length_to_percent(&value).unwrap_or(value);
        attrs.push((key, value));
    }
    attrs
}

/// Converts a length given as a fraction of the text block (`0.5\textwidth`) into a percentage
/// (`50%`). Returns `None` for absolute lengths or a value that lacks a leading digit.
pub(super) fn latex_length_to_percent(value: &str) -> Option<String> {
    let value = value.trim();
    let number = ["\\textwidth", "\\linewidth", "\\textheight"]
        .into_iter()
        .find_map(|unit| value.strip_suffix(unit))?
        .trim();
    if !number.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let (int_part, frac_part) = number.split_once('.').unwrap_or((number, ""));
    if int_part.is_empty()
        || !int_part.bytes().all(|b| b.is_ascii_digit())
        || !frac_part.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let mut digits = format!("{int_part}{frac_part}");
    let point = int_part.len() + 2;
    while digits.len() < point {
        digits.push('0');
    }
    let (whole, frac) = digits.split_at_checked(point)?;
    let whole = whole.trim_start_matches('0');
    let whole = if whole.is_empty() { "0" } else { whole };
    let frac = frac.trim_end_matches('0');
    Some(if frac.is_empty() {
        format!("{whole}%")
    } else {
        format!("{whole}.{frac}%")
    })
}

/// Splits `text` on `sep`, ignoring separators nested inside `{…}`.
pub(super) fn split_top_level(text: &str, sep: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut current = String::new();
    for c in text.chars() {
        match c {
            '{' => {
                depth += 1;
                current.push(c);
            }
            '}' => {
                if depth > 0 {
                    depth -= 1;
                }
                current.push(c);
            }
            _ if c == sep && depth == 0 => {
                parts.push(std::mem::take(&mut current));
            }
            _ => current.push(c),
        }
    }
    parts.push(current);
    parts
}

/// Parses a tabular column specification into per-column alignments, skipping rules (`|`), inter-
/// column material (`@{…}`, `!{…}`, `>{…}`, `<{…}`), and paragraph-column widths.
pub(super) fn parse_column_spec(spec: &str) -> Vec<Alignment> {
    let mut aligns = Vec::new();
    let chars: Vec<char> = spec.chars().collect();
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        match c {
            'l' => aligns.push(Alignment::AlignLeft),
            'r' => aligns.push(Alignment::AlignRight),
            'c' => aligns.push(Alignment::AlignCenter),
            'p' | 'm' | 'b' | 'X' => {
                aligns.push(Alignment::AlignLeft);
                i = skip_brace_group(&chars, i + 1);
                continue;
            }
            '@' | '!' | '>' | '<' => {
                i = skip_brace_group(&chars, i + 1);
                continue;
            }
            '*' => {
                // `*{n}{cols}` repetition is not expanded; its groups are skipped.
                i = skip_brace_group(&chars, i + 1);
                i = skip_brace_group(&chars, i);
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    aligns
}

/// Returns the index just past a `{…}` group starting at or after `start` (skipping leading spaces).
pub(super) fn skip_brace_group(chars: &[char], start: usize) -> usize {
    let mut i = start;
    while matches!(chars.get(i), Some(' ')) {
        i += 1;
    }
    if chars.get(i) != Some(&'{') {
        return i;
    }
    let mut depth = 0i32;
    while let Some(&c) = chars.get(i) {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    i
}

/// Builds a table from a column spec and the raw tabular body: rows are split on `\\`, cells on `&`.
/// The rows before the first interior horizontal rule become the header; the rest form one body.
pub(super) fn build_table(
    parser: &Parser,
    aligns: &[Alignment],
    body: &str,
    captions: bool,
) -> Block {
    let col_specs: Vec<ColSpec> = aligns
        .iter()
        .map(|a| ColSpec {
            align: a.clone(),
            width: ColWidth::ColWidthDefault,
        })
        .collect();
    let ncols = col_specs.len();

    // The first row becomes a header exactly when a horizontal rule immediately follows it.
    let mut rows: Vec<Row> = Vec::new();
    let mut caption = Caption::default();
    let mut attributes = Attr::default();
    let mut rule_pending = false;
    let mut first_row_ruled = false;
    for chunk in split_rows(body) {
        let (leading_rule, mut content) = strip_leading_rules(&chunk);
        if captions && (content.starts_with("\\caption") || content.starts_with("\\label")) {
            let mut source = parser.child(&content, false);
            loop {
                source.skip_block_ws();
                if source.at_control_word("caption") {
                    source.consume_control_word();
                    source.skip_block_ws();
                    if source.cur() == Some('*') {
                        source.bump();
                        attributes.classes.push("unnumbered".into());
                        source.skip_block_ws();
                    }
                    let short = source
                        .read_optional_raw()
                        .map(|text| source.parse_fragment(&text));
                    source.skip_block_ws();
                    let mut identifier = None;
                    let inlines = source.parse_group_inlines_capturing_label(&mut identifier);
                    if caption.long.is_empty() {
                        caption.short = short;
                    }
                    caption.long.push(Block::Plain(inlines));
                    if let Some(identifier) = identifier {
                        attributes.id = identifier.into();
                    }
                } else if source.at_control_word("label") {
                    source.consume_control_word();
                    source.skip_block_ws();
                    if let Some(identifier) = source.read_group_raw() {
                        attributes.id = identifier.into();
                    }
                } else {
                    break;
                }
            }
            content.clear();
            while let Some(character) = source.bump() {
                content.push(character);
            }
        }
        if content.trim().is_empty() {
            rule_pending |= leading_rule;
            continue;
        }
        rule_pending |= leading_rule;
        if rows.len() == 1 && rule_pending {
            first_row_ruled = true;
        }
        rule_pending = false;
        let content = split_top_level(&content, '\n').join(" ");
        rows.push(build_row(parser, &strip_rules(&content), ncols));
    }
    if rows.len() == 1 && rule_pending {
        first_row_ruled = true;
    }

    let (mut head_rows, mut body_rows) = if first_row_ruled && !rows.is_empty() {
        let body = rows.split_off(1);
        (rows, body)
    } else {
        (Vec::new(), rows)
    };
    apply_row_spans(&mut head_rows, ncols);
    apply_row_spans(&mut body_rows, ncols);

    let table = Table {
        attr: attributes,
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
    };
    Block::Table(Box::new(table))
}

fn split_rows(body: &str) -> Vec<String> {
    let mut rows = Vec::new();
    let mut row = String::new();
    let mut depth = 0usize;
    let mut characters = body.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\\'
            && let Some(next) = characters.next()
        {
            if next == '\\' && depth == 0 {
                rows.push(std::mem::take(&mut row));
            } else {
                row.push(character);
                row.push(next);
            }
            continue;
        }
        match character {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            _ => {}
        }
        row.push(character);
    }
    rows.push(row);
    rows
}

/// Builds a table row from raw cell source, splitting on `&` and padding to `ncols` columns.
/// A `\multicolumn{n}{align}{content}` field yields a single cell spanning `n` columns.
pub(super) fn build_row(parser: &Parser, source: &str, ncols: usize) -> Row {
    let mut cells = Vec::new();
    let mut span_total: i32 = 0;
    for cell_src in split_top_level(source, '&') {
        let trimmed = cell_src.trim();
        let (align, col_span, content_src) = match parse_multicolumn(trimmed) {
            Some((n, align, content)) => (align, n, content),
            None => (Alignment::AlignDefault, 1, trimmed.to_owned()),
        };
        let (row_span, content_src) =
            parse_multirow(parser, content_src.trim()).unwrap_or((1, content_src));
        let inlines = parse_cell_inlines(parser, content_src.trim());
        let content = if inlines.is_empty() {
            Vec::new()
        } else {
            vec![Block::Plain(inlines)]
        };
        cells.push(Cell {
            attr: Attr::default(),
            align,
            row_span,
            col_span: i64::from(col_span),
            content,
        });
        span_total = span_total.saturating_add(col_span.max(1));
    }
    while span_total < i32::try_from(ncols).unwrap_or(i32::MAX) {
        cells.push(Cell {
            attr: Attr::default(),
            align: Alignment::AlignDefault,
            row_span: 1,
            col_span: 1,
            content: Vec::new(),
        });
        span_total += 1;
    }
    Row {
        attr: Attr::default(),
        cells,
    }
}

fn parse_multirow(parser: &Parser, source: &str) -> Option<(i64, String)> {
    if !source.starts_with("\\multirow") {
        return None;
    }
    let mut cell = parser.child(source, false);
    if !cell.at_control_word("multirow") {
        return None;
    }
    cell.consume_control_word();
    cell.skip_block_ws();
    let _ = cell.read_optional_raw();
    cell.skip_block_ws();
    let span = cell.read_group_raw()?.trim().parse().unwrap_or(1);
    cell.skip_block_ws();
    let _ = cell.read_optional_raw();
    cell.skip_block_ws();
    let _ = cell.read_group_raw()?;
    cell.skip_block_ws();
    let _ = cell.read_optional_raw();
    cell.skip_block_ws();
    let mut content = cell.read_group_raw()?;
    while let Some(character) = cell.bump() {
        content.push(character);
    }
    Some((span, content))
}

fn apply_row_spans(rows: &mut [Row], columns: usize) {
    if !rows
        .iter()
        .flat_map(|row| &row.cells)
        .any(|cell| cell.row_span != 1)
    {
        return;
    }
    for index in 0..rows.len() {
        let (earlier, current) = rows.split_at_mut(index);
        let Some(row) = current.first_mut() else {
            continue;
        };
        let mut column = 0usize;
        for cell in &mut row.cells {
            let width = usize::try_from(cell.col_span).unwrap_or(usize::MAX).max(1);
            if cell.row_span < 0 {
                let height = usize::try_from(cell.row_span.unsigned_abs())
                    .unwrap_or(usize::MAX)
                    .min(index.saturating_add(1));
                let target = index.saturating_add(1).saturating_sub(height);
                if let Some(row) = earlier.get_mut(target)
                    && place_upward_cell(row, column, width, height, cell)
                {
                    cell.content.clear();
                }
                cell.row_span = 1;
            }
            column = column.saturating_add(width);
        }
    }
    let mut occupied = vec![0i64; columns];
    let count = rows.len();
    for (index, row) in rows.iter_mut().enumerate() {
        let remaining = i64::try_from(count - index).unwrap_or(i64::MAX);
        let mut column = 0usize;
        row.cells.retain_mut(|cell| {
            let width = usize::try_from(cell.col_span).unwrap_or(usize::MAX).max(1);
            let covered = occupied.get(column).is_some_and(|&left| left > 0);
            cell.row_span = cell.row_span.max(1).min(remaining);
            if !covered {
                for left in occupied.iter_mut().skip(column).take(width) {
                    *left = cell.row_span;
                }
            }
            column = column.saturating_add(width);
            !covered || !cell.content.is_empty()
        });
        for left in &mut occupied {
            *left = left.saturating_sub(1);
        }
    }
}

fn place_upward_cell(
    row: &mut Row,
    column: usize,
    width: usize,
    height: usize,
    source: &Cell,
) -> bool {
    let mut position = 0usize;
    let mut start = None;
    for (index, cell) in row.cells.iter().enumerate() {
        if position == column {
            start = Some(index);
        }
        if start.is_some() && (!cell.content.is_empty() || cell.row_span != 1) {
            return false;
        }
        position =
            position.saturating_add(usize::try_from(cell.col_span).unwrap_or(usize::MAX).max(1));
        if position == column.saturating_add(width)
            && let Some(start) = start
        {
            let mut cell = source.clone();
            cell.row_span = i64::try_from(height).unwrap_or(i64::MAX);
            row.cells.splice(start..=index, [cell]);
            return true;
        }
        if position > column.saturating_add(width) {
            break;
        }
    }
    false
}

/// Parses a `\multicolumn{n}{align}{content}` cell, returning the span, cell alignment, and the
/// raw content. Returns `None` when the field is not a multicolumn.
pub(super) fn parse_multicolumn(src: &str) -> Option<(i32, Alignment, String)> {
    let rest = src.strip_prefix("\\multicolumn")?;
    let chars: Vec<char> = rest.chars().collect();
    let (span, next) = read_brace_group(&chars, 0)?;
    let (align_spec, next) = read_brace_group(&chars, next)?;
    let (content, _) = read_brace_group(&chars, next)?;
    let span: i32 = span.trim().parse().ok()?;
    if span < 1 {
        return None;
    }
    let align = parse_column_spec(&align_spec)
        .into_iter()
        .next()
        .unwrap_or(Alignment::AlignDefault);
    Some((span, align, content))
}

/// Reads a balanced `{…}` group at or after `start` (skipping leading spaces), returning its inner
/// content and the index just past the closing brace.
pub(super) fn read_brace_group(chars: &[char], start: usize) -> Option<(String, usize)> {
    let mut i = start;
    while matches!(chars.get(i), Some(' ')) {
        i += 1;
    }
    if chars.get(i) != Some(&'{') {
        return None;
    }
    let mut depth = 0i32;
    let mut content = String::new();
    while let Some(&c) = chars.get(i) {
        match c {
            '{' => {
                depth += 1;
                if depth > 1 {
                    content.push(c);
                }
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((content, i + 1));
                }
                content.push(c);
            }
            _ => content.push(c),
        }
        i += 1;
    }
    None
}

/// Strips leading whitespace and horizontal-rule commands from a row chunk, returning whether a
/// header-separating rule was present and the remaining content.
pub(super) fn strip_leading_rules(chunk: &str) -> (bool, String) {
    let chars: Vec<char> = chunk.chars().collect();
    let mut i = 0;
    let mut header_boundary = false;
    loop {
        while matches!(chars.get(i), Some(c) if c.is_whitespace()) {
            i += 1;
        }
        match rule_command_at(&chars, i) {
            Some((end, header)) => {
                header_boundary |= header;
                i = end;
            }
            None => break,
        }
    }
    (
        header_boundary,
        chars.get(i..).unwrap_or(&[]).iter().collect(),
    )
}

/// If a horizontal-rule command (`\hline`, `\toprule`, …) begins at `chars[start]`, returns the index
/// just past the command name and all its bracketed arguments, together with whether the rule marks a
/// header boundary. A dashed or custom rule (`\hdashline`, `\specialrule`) is removed from the source
/// but does not separate the header row from the body.
pub(super) fn rule_command_at(chars: &[char], start: usize) -> Option<(usize, bool)> {
    if chars.get(start) != Some(&'\\') {
        return None;
    }
    let mut j = start + 1;
    let mut name = String::new();
    while let Some(&d) = chars.get(j) {
        if d.is_ascii_alphabetic() {
            name.push(d);
            j += 1;
        } else {
            break;
        }
    }
    if !is_rule_command(&name) {
        return None;
    }
    let header_boundary = !matches!(name.as_str(), "hdashline" | "specialrule");
    while matches!(chars.get(j), Some('{' | '[' | '(')) {
        j = skip_rule_argument(chars, j);
    }
    Some((j, header_boundary))
}

pub(super) fn is_rule_command(name: &str) -> bool {
    matches!(
        name,
        "hline"
            | "toprule"
            | "midrule"
            | "bottomrule"
            | "cmidrule"
            | "cline"
            | "hdashline"
            | "specialrule"
    )
}

/// Returns the index just past a bracketed argument (`{…}`, `[…]`, or `(…)`) starting at `start`.
pub(super) fn skip_rule_argument(chars: &[char], start: usize) -> usize {
    let close = match chars.get(start) {
        Some('{') => '}',
        Some('[') => ']',
        Some('(') => ')',
        _ => return start,
    };
    let mut i = start + 1;
    while let Some(&c) = chars.get(i) {
        i += 1;
        if c == close {
            break;
        }
    }
    i
}

/// Parses a single table cell's source into inlines using a fresh sub-parser.
pub(super) fn parse_cell_inlines(parser: &Parser, source: &str) -> Vec<Inline> {
    let mut sub = parser.child(source, false);
    trim_inlines(sub.parse_inlines(InlineStop::Paragraph))
}

/// Removes horizontal-rule commands (`\hline`, `\toprule`, …, `\cline{…}`) from a table row source.
pub(super) fn strip_rules(row: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = row.chars().collect();
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        if let Some((end, _)) = rule_command_at(&chars, i) {
            i = end;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}
