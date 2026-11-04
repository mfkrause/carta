use super::attributes::{add_block_attr, block_attributes, block_attributes_at};
use super::block::physical_blank_line;
use super::{Attributes, Parser, ParserError, ParserSnapshot};
use carta_ast::{Alignment, Attr, Block, Cell, ColSpec, ColWidth, Row, Table, TableBody};

#[derive(Clone, Default)]
struct Column {
    align: Option<Alignment>,
    width: Option<i64>,
    style: char,
}

#[derive(Clone, Default)]
struct CellSpec {
    align: Option<Alignment>,
    style: char,
    columns: i64,
    rows: i64,
    repeat: usize,
}

struct PlannedCell {
    spec: CellSpec,
    style: char,
    text: String,
}

impl Parser {
    #[allow(
        clippy::too_many_lines,
        reason = "Table syntax and row assembly share column state."
    )]
    pub(super) fn table(
        &mut self,
        lines: &[&str],
        attributes: &Attributes,
        border: &str,
        closing_prefix: Option<&str>,
        section: Option<i64>,
    ) -> Option<Block> {
        let explicit_columns = attributes.get("cols").is_some();
        let mut columns = attributes.get("cols").map_or_else(Vec::new, columns);
        if explicit_columns && columns.is_empty() && attributes.get("cols") == Some("") {
            columns.push(Column::default());
        }
        let border_syntax = match border.chars().next() {
            Some(',') => "csv",
            Some(':') => "dsv",
            _ => "psv",
        };
        let syntax = match attributes.get("format") {
            Some("psv") => "psv",
            Some("csv") => "csv",
            Some("dsv") => "dsv",
            Some("tsv") => "tsv",
            _ => border_syntax,
        };
        let separator = attributes
            .get("separator")
            .and_then(|s| s.chars().next())
            .unwrap_or(match syntax {
                "csv" => ',',
                "tsv" => '\t',
                "dsv" => ':',
                _ => '|',
            });
        let leading = lines
            .iter()
            .take_while(|line| super::blank_line(line))
            .count();
        let mut content = format!("{}\n", lines.get(leading..).unwrap_or_default().join("\n"));
        if let Some(prefix) = closing_prefix.filter(|prefix| !prefix.is_empty()) {
            content.push_str(prefix);
        }
        let mut rows = Vec::new();
        if syntax == "psv" {
            let mut pieces = psv_cells(&content, separator);
            let first = super::trim_space_start(&content);
            if !first.is_empty()
                && !first
                    .find(separator)
                    .is_some_and(|index| valid_cell_spec(super::trim_space_start(&first[..index])))
            {
                return None;
            }
            if pieces.is_empty() && !super::trim_space(&content).is_empty() {
                return None;
            }
            let mut plans = Vec::new();
            if !explicit_columns && !pieces.is_empty() {
                let first_line = pieces.first().map_or(0, |(_, _, line)| *line);
                let count = pieces
                    .iter()
                    .take_while(|(_, _, line)| *line == first_line)
                    .map(|(spec, _, _)| {
                        cell_spec(spec).repeat.saturating_mul(
                            usize::try_from(cell_spec(spec).columns).unwrap_or(usize::MAX),
                        )
                    })
                    .fold(0usize, usize::saturating_add);
                if count > 10000 {
                    self.fatal_error(carta_core::Error::OutputTooLarge);
                    return Some(Block::Table(Box::default()));
                }
                if count == 0 {
                    plans.push(Vec::new());
                    pieces.clear();
                }
                columns = vec![Column::default(); count];
            }
            if columns.is_empty() && !pieces.is_empty() {
                return None;
            }
            match plan_psv_rows(&pieces, &columns, &mut plans) {
                Ok(()) => {}
                Err(PlanError::Invalid) => return None,
                Err(PlanError::TooLarge) => {
                    self.fatal_error(carta_core::Error::OutputTooLarge);
                    return Some(Block::Table(Box::default()));
                }
            }
            let snapshot = self.snapshot();
            for planned in plans {
                let mut row = Row::default();
                for planned in planned {
                    row.cells.push(Cell {
                        attr: Attr::default(),
                        align: planned.spec.align.unwrap_or(Alignment::AlignDefault),
                        row_span: planned.spec.rows,
                        col_span: planned.spec.columns,
                        content: self.cell_content(&planned.text, planned.style, section),
                    });
                    if self.error.is_some() {
                        break;
                    }
                }
                rows.push(row);
                if self.error.is_some() {
                    break;
                }
            }
            if self.rollback_failed_parse(snapshot) {
                return None;
            }
        } else {
            let values = delimited_rows(&content, separator, syntax != "dsv")?;
            let snapshot = self.snapshot();
            for values in values {
                let mut row = Row::default();
                for (index, value) in values.into_iter().enumerate() {
                    row.cells.push(Cell {
                        attr: Attr::default(),
                        align: Alignment::AlignDefault,
                        row_span: 1,
                        col_span: 1,
                        content: self.cell_content(
                            &value,
                            columns.get(index).map_or('\0', |c| c.style),
                            section,
                        ),
                    });
                    if self.error.is_some() {
                        break;
                    }
                }
                if !row.cells.is_empty() {
                    rows.push(row);
                }
                if self.error.is_some() {
                    break;
                }
            }
            if self.rollback_failed_parse(snapshot) {
                return None;
            }
            if columns.is_empty() {
                columns = vec![Column::default(); rows.first().map_or(0, |r| r.cells.len())];
            }
        }
        if rows.is_empty()
            && closing_prefix
                .is_some_and(|prefix| !prefix.is_empty() && super::trim_space(prefix).is_empty())
        {
            return None;
        }
        if rows.is_empty() {
            columns.clear();
        }
        let column_count = columns.len();
        let mut table = Table::default();
        let total = columns
            .iter()
            .map(|column| column.width.unwrap_or(1))
            .fold(0_i64, i64::wrapping_add);
        table.col_specs = columns
            .into_iter()
            .map(|c| ColSpec {
                align: c.align.unwrap_or(Alignment::AlignDefault),
                width: c.width.map_or(ColWidth::ColWidthDefault, |width| {
                    column_width(width, total)
                }),
            })
            .collect();
        let options = attributes
            .get("options")
            .unwrap_or("")
            .split(super::is_space)
            .filter(|word| !word.is_empty())
            .collect::<Vec<_>>();
        if (options.contains(&"header") || !options.contains(&"noheader")) && !rows.is_empty() {
            table.head.rows.push(rows.remove(0));
        }
        if (options.contains(&"footer") || !options.contains(&"nofooter")) && !rows.is_empty() {
            table.foot.rows.push(rows.pop().unwrap_or_default());
        }
        normalize_rows(&mut table.head.rows, column_count);
        normalize_rows(&mut rows, column_count);
        normalize_rows(&mut table.foot.rows, column_count);
        table.bodies.push(TableBody {
            body: rows,
            ..Default::default()
        });
        Some(Block::Table(Box::new(table)))
    }

    fn cell_content(&mut self, text: &str, style: char, section: Option<i64>) -> Vec<Block> {
        match style {
            'a' => {
                let depth = self.depth;
                let hard_breaks = self.hard_breaks;
                self.depth = 0;
                self.include_depth += 1;
                let blocks = self.document(super::trim_space(text)).blocks;
                self.include_depth -= 1;
                self.depth = depth;
                self.hard_breaks = hard_breaks;
                blocks
            }
            'l' => vec![Block::CodeBlock(Box::default(), text.into())],
            'e' | 'h' | 'm' | 's' => self.block_sequence(
                &super::trim_space(text).lines().collect::<Vec<_>>(),
                section,
                true,
                style,
            ),
            _ => self.default_cell_content(text, section),
        }
    }

    pub(super) fn rollback_failed_parse(&mut self, snapshot: ParserSnapshot) -> bool {
        let Some(error) = self.error.take() else {
            return false;
        };
        self.restore(snapshot);
        if let ParserError::Fatal(_) = error {
            self.error = Some(error);
        }
        true
    }

    #[allow(
        clippy::too_many_lines,
        reason = "Cell paragraphs share pending titles and attributes."
    )]
    fn default_cell_content(&mut self, text: &str, section: Option<i64>) -> Vec<Block> {
        let lines: Vec<_> = super::trim_space(text).lines().collect();
        let mut output = Vec::new();
        let mut position = 0;
        let mut pending_attributes = Attributes::default();
        let mut pending_title: Option<&str> = None;
        while position < lines.len() {
            while lines
                .get(position)
                .is_some_and(|line| physical_blank_line(line))
            {
                position += 1;
            }
            if position == lines.len() {
                break;
            }
            if self.depth > 1 && lines.get(position) == Some(&"+") {
                position += 1;
                continue;
            }

            if let Some(id) = lines.get(position).and_then(|line| block_anchor(line)) {
                pending_attributes
                    .named
                    .insert("id".to_owned(), id.to_owned());
                position += 1;
                continue;
            }
            if let Some((content, count)) = block_attributes_at(&lines, position) {
                pending_attributes.extend(&content);
                position += count;
                continue;
            }
            if lines
                .get(position)
                .is_some_and(|line| block_attributes(line).is_some())
            {
                return output;
            }
            if let Some(content) = lines
                .get(position)
                .and_then(|line| line.strip_prefix('.'))
                .filter(|content| !content.starts_with([' ', '.']))
            {
                pending_title = Some(content);
                position += 1;
                continue;
            }
            if pending_attributes.style() == "comment" {
                return output;
            }
            if lines
                .get(position)
                .is_some_and(|line| super::line_comment(line))
            {
                position += 1;
                continue;
            }
            if let Some(delimiter) = lines
                .get(position)
                .copied()
                .filter(|line| line.starts_with("////") && line.chars().all(|c| c == '/'))
            {
                position += 1;
                while position < lines.len() && lines.get(position) != Some(&delimiter) {
                    position += 1;
                }
                if position == lines.len() {
                    return output;
                }
                position += 1;
                if lines
                    .get(position)
                    .is_some_and(|line| physical_blank_line(line))
                {
                    return output;
                }
                continue;
            }
            if section.is_some()
                && lines
                    .get(position)
                    .is_some_and(|line| section_heading(line))
            {
                return output;
            }

            let start = position;
            while let Some(line) = lines.get(position) {
                if physical_blank_line(line)
                    || self.depth > 1 && *line == "+"
                    || block_attributes_at(&lines, position).is_some()
                    || block_attributes(line).is_some()
                    || line.starts_with("////") && line.chars().all(|c| c == '/')
                {
                    break;
                }
                position += 1;
            }

            let paragraph_lines = lines.get(start..position).unwrap_or_default();
            let content = paragraph_lines.join("\n");
            let attributes = std::mem::take(&mut pending_attributes);
            let title = pending_title.take().map(|title| self.inlines(title));
            let hard_breaks = self.hard_breaks || attributes.get("options") == Some("hardbreaks");
            let mut block = if let Some((kind, body)) = super::block::admonition(&content) {
                self.admonition(kind, body, hard_breaks)
            } else {
                Block::Para(self.paragraph_inlines(&content, hard_breaks))
            };
            if let Some(title) = title {
                block = super::block::add_title(block, title);
            }
            output.push(add_block_attr(
                block,
                attributes.attr(),
                attributes.has_empty_id(),
            ));
        }
        output
    }
}

fn section_heading(line: &str) -> bool {
    let line = super::trim_space_start(line);
    let Some(first) = line.chars().next().filter(|c| matches!(c, '=' | '#')) else {
        return false;
    };
    let count = line.chars().take_while(|c| *c == first).count();
    (1..=6).contains(&count) && line.get(count..).is_some_and(|text| text.starts_with(' '))
}

fn block_anchor(line: &str) -> Option<&str> {
    let source = line.strip_prefix("[[")?;
    let end = source.find(|character: char| super::is_space(character) || character == ']')?;
    if end == 0 || source.get(end..)? != "]]" {
        return None;
    }
    source.get(..end)
}

fn psv_cells(content: &str, separator: char) -> Vec<(String, String, usize)> {
    let mut output = Vec::new();
    let mut previous = 0;
    let mut specification = String::new();
    let mut previous_line = 0;
    let mut line = 0;
    let escaped_separator = format!("\\{separator}");
    let separator_text = separator.to_string();
    for (position, character) in content.char_indices() {
        if character != separator || content[..position].ends_with('\\') {
            continue;
        }
        let prefix = &content[previous..position];
        let spec_start = prefix.rfind(super::is_space).map_or(0, |p| p + 1);
        let candidate = &prefix[spec_start..];
        let suffix = candidate
            .char_indices()
            .map(|(index, _)| index)
            .chain(std::iter::once(candidate.len()))
            .find(|index| valid_cell_spec(&candidate[*index..]))
            .unwrap_or(candidate.len());
        let specification_start = spec_start + suffix;
        let text = &prefix[..specification_start];
        let next_spec = &prefix[specification_start..];
        if previous != 0 {
            output.push((
                std::mem::take(&mut specification),
                text.replace(&escaped_separator, &separator_text),
                previous_line,
            ));
        }
        if previous != 0 && text.ends_with('\n') {
            line += 1;
        }
        next_spec.clone_into(&mut specification);
        previous = position + character.len_utf8();
        previous_line = line;
    }
    if previous != 0 {
        output.push((
            specification,
            content[previous..].replace(&escaped_separator, &separator_text),
            previous_line,
        ));
    }
    output
}

enum PlanError {
    Invalid,
    TooLarge,
}

fn plan_psv_rows(
    pieces: &[(String, String, usize)],
    columns: &[Column],
    plans: &mut Vec<Vec<PlannedCell>>,
) -> Result<(), PlanError> {
    if columns.is_empty() {
        return if pieces.is_empty() {
            Ok(())
        } else {
            Err(PlanError::Invalid)
        };
    }

    let mut rowspans = vec![0i64; columns.len()];
    let mut piece_index = 0;
    while piece_index < pieces.len() {
        if plans.len() >= 10000 {
            return Err(PlanError::TooLarge);
        }
        let available = rowspans
            .iter()
            .take(columns.len())
            .enumerate()
            .filter_map(|(index, rowspan)| (*rowspan <= 0).then_some(index))
            .collect::<Vec<_>>();
        if available.is_empty() {
            plans.push(Vec::new());
            update_rowspans(&mut rowspans, columns.len(), &[]);
            continue;
        }

        let mut row = Vec::new();
        let mut used_columns = 0usize;
        while let Some((specification, text, _)) = pieces.get(piece_index) {
            if used_columns >= available.len() {
                break;
            }
            let spec = cell_spec(specification);
            let span = usize::try_from(spec.columns).unwrap_or(usize::MAX);
            for _ in 0..spec.repeat {
                let style_column = available.get(used_columns).copied();
                let style = if spec.style == '\0' {
                    style_column
                        .and_then(|index| columns.get(index))
                        .map_or('\0', |column| column.style)
                } else {
                    spec.style
                };
                row.push(PlannedCell {
                    spec: spec.clone(),
                    style,
                    text: text.clone(),
                });
                used_columns = used_columns.saturating_add(span);
            }
            piece_index += 1;
        }
        if used_columns < available.len() {
            return Err(PlanError::Invalid);
        }
        update_rowspans(&mut rowspans, columns.len(), &row);
        plans.push(row);
    }
    Ok(())
}

fn update_rowspans(rowspans: &mut [i64], columns: usize, row: &[PlannedCell]) {
    for rowspan in rowspans.iter_mut().take(columns) {
        *rowspan = rowspan.saturating_sub(1);
    }
    let mut column = 0usize;
    for cell in row {
        let span = usize::try_from(cell.spec.columns).unwrap_or(usize::MAX);
        for rowspan in rowspans.iter_mut().skip(column).take(span) {
            *rowspan = rowspan.saturating_add(cell.spec.rows);
        }
        column = column.saturating_add(span);
    }
}

#[allow(
    clippy::cast_precision_loss,
    reason = "Pandoc normalizes integer column widths through floating point."
)]
fn column_width(width: i64, total: i64) -> ColWidth {
    ColWidth::ColWidth(width as f64 / total as f64)
}

fn delimited_rows(content: &str, separator: char, quoted: bool) -> Option<Vec<Vec<String>>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut characters = content.char_indices().peekable();
    let mut in_quote = false;
    let mut row_content = false;
    let mut field_seen = false;
    while let Some((position, character)) = characters.next() {
        if !in_quote
            && row.is_empty()
            && super::trim_space(&field).is_empty()
            && super::line_comment(&content[position..])
        {
            for (_, next) in characters.by_ref() {
                if next == '\n' {
                    break;
                }
            }
            field.clear();
            field_seen = false;
        } else if !quoted && character == '\\' {
            field.push(character);
            if let Some((_, next)) = characters.next() {
                field.push(next);
                field_seen = true;
            }
        } else if quoted && character == '"' && (in_quote || field.trim_matches(' ').is_empty()) {
            if in_quote {
                if characters.peek().is_some_and(|(_, next)| *next == '"') {
                    return None;
                }
                let rest = &content[position + 1..];
                if !rest.starts_with([separator, '\n', '\r'])
                    && !rest.is_empty()
                    && !super::trim_space(rest.split('\n').next().unwrap_or_default()).is_empty()
                {
                    return None;
                }
                in_quote = false;
            } else {
                in_quote = true;
                field_seen = true;
                row_content = true;
            }
        } else if !in_quote && (character == separator || character == '\n') {
            if quoted && !field_seen && (character == separator || row_content) {
                return None;
            }
            row_content |= character == separator;
            if field_seen {
                row.push(super::trim_space(&field).replace("\"\"", "\""));
            }
            field.clear();
            field_seen = false;
            if character == '\n' {
                if row_content || row.iter().any(|field| !field.is_empty()) {
                    rows.push(std::mem::take(&mut row));
                } else {
                    row.clear();
                }
                row_content = false;
            }
        } else {
            field.push(character);
            if !quoted || in_quote || character != ' ' {
                field_seen = true;
            }
            if !super::is_space(character) {
                row_content = true;
            }
        }
    }
    if in_quote {
        return None;
    }
    Some(rows)
}

fn columns(mut source: &str) -> Vec<Column> {
    let mut columns = Vec::new();
    if source.is_empty() {
        return vec![Column::default()];
    }
    loop {
        let (repeat, rest) = multiplier(source);
        source = rest;
        let align = alignment(source);
        if align.is_some() {
            source = &source[1..];
        }
        if source.starts_with('.') && source.get(1..).is_some_and(|s| alignment(s).is_some()) {
            source = &source[2..];
        }
        let digits = source.chars().take_while(char::is_ascii_digit).count();
        let width = super::unsigned(&source[..digits], 10).map(u64::cast_signed);
        source = &source[digits..];
        if source.starts_with(['~', '%']) {
            source = &source[1..];
        }
        let style = source
            .chars()
            .next()
            .filter(|c| "adehmls".contains(*c))
            .unwrap_or_default();
        if style != '\0' {
            source = &source[1..];
        }
        columns.extend(std::iter::repeat_n(
            Column {
                align,
                width,
                style,
            },
            repeat,
        ));
        if let Some(rest) = source.strip_prefix(',') {
            source = super::trim_space_start(rest);
            if source.is_empty() {
                columns.push(Column::default());
                break;
            }
        } else {
            break;
        }
        if source.is_empty() {
            break;
        }
    }
    columns
}
fn multiplier(source: &str) -> (usize, &str) {
    let digits = source.chars().take_while(char::is_ascii_digit).count();
    if source[digits..].starts_with('*') {
        (
            source[..digits].parse::<usize>().unwrap_or(1).min(10000),
            &source[digits + 1..],
        )
    } else {
        (1, source)
    }
}
fn alignment(source: &str) -> Option<Alignment> {
    match source.chars().next()? {
        '<' => Some(Alignment::AlignLeft),
        '^' => Some(Alignment::AlignCenter),
        '>' => Some(Alignment::AlignRight),
        _ => None,
    }
}
fn cell_spec(source: &str) -> CellSpec {
    let (repeat, source) = source.split_once('*').map_or((1, source), |(n, s)| {
        (n.parse::<usize>().unwrap_or(1).min(10000), s)
    });
    let (columns, rows, source) = if let Some((span, rest)) = source.split_once('+') {
        let (columns, rows) = span.split_once('.').map_or((span, ""), |v| v);
        (
            columns.parse::<i64>().unwrap_or(1).max(0),
            rows.parse::<i64>().unwrap_or(1).max(1),
            rest,
        )
    } else {
        (1, 1, source)
    };
    CellSpec {
        repeat,
        columns,
        rows,
        align: alignment(source),
        style: source
            .chars()
            .find(|c| "adehmls".contains(*c))
            .unwrap_or_default(),
    }
}

fn normalize_rows(rows: &mut [Row], columns: usize) {
    let count = rows.len();
    let mut overhang = vec![1i64; columns];
    for (index, row) in rows.iter_mut().enumerate() {
        let remaining_rows = i64::try_from(count - index).unwrap_or(i64::MAX);
        let mut source = std::mem::take(&mut row.cells).into_iter();
        let mut cells = Vec::new();
        let mut next_overhang = Vec::with_capacity(columns);
        let mut column = 0;
        while column < columns {
            let previous_overhang = overhang.get(column).copied().unwrap_or(1);
            if previous_overhang > 1 {
                next_overhang.push(previous_overhang - 1);
                column += 1;
                continue;
            }

            let mut cell = source.next().unwrap_or_else(|| Cell {
                attr: Attr::default(),
                align: Alignment::AlignDefault,
                row_span: 1,
                col_span: 1,
                content: vec![],
            });
            cell.row_span = cell.row_span.min(remaining_rows).max(1);
            let width = usize::try_from(cell.col_span.max(1))
                .unwrap_or(usize::MAX)
                .min(columns.saturating_sub(column));
            let fit = overhang
                .iter()
                .skip(column)
                .take(width)
                .take_while(|height| **height == 1)
                .count()
                .max(1);
            cell.col_span = i64::try_from(fit).unwrap_or(i64::MAX);
            next_overhang.extend(std::iter::repeat_n(cell.row_span, fit));
            column += fit;
            cells.push(cell);
        }
        row.cells = cells;
        overhang = next_overhang;
    }
}

fn valid_cell_spec(mut source: &str) -> bool {
    let (_, rest) = multiplier(source);
    source = rest;
    let original = source;
    let columns = source.chars().take_while(char::is_ascii_digit).count();
    source = &source[columns..];
    let mut rows = 0;
    if let Some(rest) = source.strip_prefix('.') {
        rows = rest.chars().take_while(char::is_ascii_digit).count();
        if rows > 0 {
            source = &rest[rows..];
        }
    }
    if (columns > 0 || rows > 0) && source.starts_with('+') {
        source = &source[1..];
    } else {
        source = original;
    }
    if alignment(source).is_some() {
        source = &source[1..];
    }
    if source.starts_with('.')
        && source
            .get(1..)
            .is_some_and(|rest| alignment(rest).is_some())
    {
        source = &source[2..];
    }
    if source
        .chars()
        .next()
        .is_some_and(|character| "adehmls".contains(character))
    {
        source = &source[1..];
    }
    source.is_empty()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::super::AsciidocReader;
    use carta_ast::{Attr, Block, Inline};
    use carta_core::{Reader, ReaderOptions};

    fn cell(source: &str) -> Vec<Block> {
        let document = AsciidocReader
            .read(source, &ReaderOptions::default())
            .unwrap();
        let Some(Block::Table(table)) = document.blocks.first() else {
            panic!("expected a table");
        };
        let Some(cell) = table.head.rows.first().and_then(|row| row.cells.first()) else {
            panic!("expected a table cell");
        };
        cell.content.clone()
    }

    #[test]
    fn default_cells_consume_comments_before_the_next_paragraph() {
        for comment in ["////\n////", "////\ntext\n////"] {
            let source = format!("[cols=1]\n|===\n|[.role]\n{comment}\n\nText\n|===\n");
            assert!(cell(&source).is_empty(), "comment: {comment}");
        }

        let line_comment = cell("[cols=1]\n|===\n|[.role]\n// text\n\nText\n|===\n");
        assert_eq!(
            line_comment,
            vec![Block::Div(
                Box::new(Attr {
                    classes: vec!["role".into()],
                    attributes: vec![("wrapper".into(), "1".into())],
                    ..Attr::default()
                }),
                vec![Block::Para(vec![Inline::Str("Text".into())])]
            )]
        );

        assert!(cell("[cols=1]\n|===\n|[.role]\n// text\n== Heading\n|===\n").is_empty());
        assert!(cell("[cols=1]\n|===\n|[.role]\n// text\n\n== Heading\n|===\n").is_empty());
    }
}
