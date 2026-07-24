//! Directive body parsing and shared attribute and table helpers.

use super::markers::field_marker;
use carta_ast::{Alignment, Attr, Block, Cell, Inline, Row};

// --- directive helpers -------------------------------------------------------------------------

/// Split a directive body into its argument (first line), its options (the immediately following
/// `:key: value` lines), and its content (everything after the blank separator).
pub(super) fn split_directive(body: &[String]) -> (String, Vec<(String, String)>, Vec<String>) {
    let mut idx = 0;
    let mut argument = String::new();
    if let Some(first) = body.first()
        && !first.is_empty()
        && option_line(first).is_none()
    {
        argument.clone_from(first);
        idx = 1;
    }
    let mut options = Vec::new();
    while let Some(line) = body.get(idx) {
        match option_line(line) {
            Some(option) => {
                options.push(option);
                idx += 1;
            }
            None => break,
        }
    }
    while body.get(idx).is_some_and(std::string::String::is_empty) {
        idx += 1;
    }
    let content = body.get(idx..).unwrap_or(&[]).to_vec();
    (argument, options, content)
}

/// The block content of a directive whose first-line text is body content rather than an argument:
/// the body with any leading option lines (and the blank line that follows them) removed.
pub(super) fn directive_content(body: &[String]) -> Vec<String> {
    let mut idx = 0;
    while body.get(idx).is_some_and(|l| option_line(l).is_some()) {
        idx += 1;
    }
    if idx > 0 {
        while body.get(idx).is_some_and(std::string::String::is_empty) {
            idx += 1;
        }
    }
    body.get(idx..).unwrap_or(&[]).to_vec()
}

/// The normalized column widths from a `:widths:` option, each as a fraction of their sum.
/// `None` when the option is absent, set to `auto`, or carries no positive numbers.
pub(super) fn directive_widths(options: &[(String, String)]) -> Option<Vec<f64>> {
    let value = options.iter().find(|(k, _)| k == "widths")?.1.trim();
    if value.is_empty() || value == "auto" {
        return None;
    }
    let nums: Vec<f64> = value
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse::<f64>().ok())
        .collect();
    let sum: f64 = nums.iter().sum();
    if nums.is_empty() || sum <= 0.0 {
        return None;
    }
    Some(nums.iter().map(|n| n / sum).collect())
}

/// The non-negative integer value of a directive option, defaulting to zero when absent or unparsable.
pub(super) fn directive_count(options: &[(String, String)], key: &str) -> usize {
    options
        .iter()
        .find(|(k, _)| k == key)
        .and_then(|(_, v)| v.trim().parse().ok())
        .unwrap_or(0)
}

/// Wrap each row's cells in a table [`Row`].
pub(super) fn cells_to_rows(rows: Vec<Vec<Cell>>) -> Vec<Row> {
    rows.into_iter()
        .map(|cells| Row {
            attr: Attr::default(),
            cells,
        })
        .collect()
}

/// Build one `list-table` row, padding short rows with empty cells and demoting a lone paragraph
/// in a cell to a plain block.
pub(super) fn list_row(cells: Vec<Vec<Block>>, num_cols: usize) -> Vec<Cell> {
    let mut row: Vec<Cell> = cells
        .into_iter()
        .map(|content| {
            let content = if let [Block::Para(_)] = content.as_slice() {
                content.into_iter().map(to_plain).collect()
            } else {
                content
            };
            Cell {
                attr: Attr::default(),
                align: Alignment::AlignDefault,
                row_span: 1,
                col_span: 1,
                content,
            }
        })
        .collect();
    while row.len() < num_cols {
        row.push(Cell {
            attr: Attr::default(),
            align: Alignment::AlignDefault,
            row_span: 1,
            col_span: 1,
            content: Vec::new(),
        });
    }
    row
}

/// Parse comma-separated values into records of trimmed fields. Fields may be double-quoted, with a
/// doubled quote denoting a literal quote; whitespace after a delimiter is ignored; and a quoted
/// field may span lines. Blank records are dropped.
pub(super) fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let chars: Vec<char> = text.chars().collect();
    let mut records: Vec<Vec<String>> = Vec::new();
    let mut record: Vec<String> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        while matches!(chars.get(i), Some(' ' | '\t')) {
            i += 1;
        }
        let mut field = String::new();
        if chars.get(i) == Some(&'"') {
            i += 1;
            loop {
                match chars.get(i) {
                    Some('"') if chars.get(i + 1) == Some(&'"') => {
                        field.push('"');
                        i += 2;
                    }
                    Some('"') => {
                        i += 1;
                        break;
                    }
                    Some(c) => {
                        field.push(*c);
                        i += 1;
                    }
                    None => break,
                }
            }
            while !matches!(chars.get(i), Some(',' | '\n') | None) {
                i += 1;
            }
        } else {
            while !matches!(chars.get(i), Some(',' | '\n') | None) {
                if let Some(c) = chars.get(i) {
                    field.push(*c);
                }
                i += 1;
            }
        }
        record.push(field.trim().to_string());
        // Comma spelled out to keep the three field terminators (separator, record break, end) together.
        #[allow(clippy::match_same_arms)]
        match chars.get(i) {
            Some(',') => i += 1,
            Some('\n') => {
                i += 1;
                records.push(std::mem::take(&mut record));
            }
            _ => i += 1,
        }
    }
    if !record.is_empty() {
        records.push(record);
    }
    records.retain(|r| !(r.len() == 1 && r.first().is_some_and(String::is_empty)));
    records
}

/// Parse a directive option line `:key: value`, returning the key and its trimmed value.
fn option_line(line: &str) -> Option<(String, String)> {
    let (name, col) = field_marker(line)?;
    let value: String = line.chars().skip(col).collect();
    Some((name, value.trim().to_string()))
}

/// Build the attributes of a code block from its language argument and options.
pub(super) fn code_attr(argument: &str, options: &[(String, String)]) -> Attr {
    let mut classes = Vec::new();
    let lang = argument.trim();
    if !lang.is_empty() {
        classes.push(lang.to_string());
    }
    let mut id = String::new();
    let mut attributes = Vec::new();
    for (key, value) in options {
        match key.as_str() {
            "name" => id.clone_from(value),
            "class" => classes.extend(value.split_whitespace().map(str::to_string)),
            // Line numbering is requested by a marker class; a non-empty value sets the first line.
            "number-lines" => {
                classes.push("numberLines".to_string());
                let start = value.trim();
                if !start.is_empty() {
                    attributes.push(("startFrom".to_string(), start.to_string()));
                }
            }
            other => attributes.push((other.to_string(), value.clone())),
        }
    }
    Attr {
        id: id.into(),
        classes: classes.into_iter().map(Into::into).collect(),
        attributes: attributes
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect(),
    }
}

/// Build the attributes, description, and destination of an image from its URI argument and options.
/// The returned classes are the plain `:class:` list; callers that render a standalone image fold
/// the alignment into them with [`image_classes`].
pub(super) fn image_parts(
    argument: &str,
    options: &[(String, String)],
) -> (Attr, Vec<Inline>, String) {
    let url = argument.split_whitespace().collect::<Vec<_>>().join("");
    let mut id = String::new();
    let mut description = Vec::new();
    for (key, value) in options {
        match key.as_str() {
            "alt" => description = vec![Inline::Str(value.clone().into())],
            "name" => id.clone_from(value),
            _ => {}
        }
    }
    (
        Attr {
            id: id.into(),
            classes: class_list(options, "class")
                .into_iter()
                .map(Into::into)
                .collect(),
            attributes: image_dimensions(options)
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        },
        description,
        url,
    )
}

/// The classes of a standalone image: the `:class:` list, repeated, with the alignment appended to
/// the last entry (or standing alone when there are no classes).
pub(super) fn image_classes(options: &[(String, String)]) -> Vec<String> {
    let classes = class_list(options, "class");
    aligned_classes(classes.clone(), classes, &align_suffix(options))
}

/// Build the attributes of a figure from its options: its `:figclass:` and `:class:` lists with the
/// alignment folded in. The figure's `:name:` identifies its image, not the figure itself.
pub(super) fn figure_attr(options: &[(String, String)]) -> Attr {
    Attr {
        id: carta_ast::Text::default(),
        classes: aligned_classes(
            class_list(options, "figclass"),
            class_list(options, "class"),
            &align_suffix(options),
        )
        .into_iter()
        .map(Into::into)
        .collect(),
        attributes: Vec::new(),
    }
}

/// The values of every option named `key`, split on whitespace, in source order.
pub(super) fn class_list(options: &[(String, String)], key: &str) -> Vec<String> {
    options
        .iter()
        .filter(|(k, _)| k == key)
        .flat_map(|(_, v)| v.split_whitespace().map(str::to_string))
        .collect()
}

/// The class an `:align:` option contributes (`align-<value>`), or empty when there is none.
fn align_suffix(options: &[(String, String)]) -> String {
    options
        .iter()
        .find(|(k, _)| k == "align")
        .map(|(_, v)| v.trim())
        .filter(|v| !v.is_empty())
        .map_or_else(String::new, |v| format!("align-{v}"))
}

/// Combine two class lists with an optional alignment class. With no alignment the lists are
/// concatenated; otherwise the alignment is appended to the last class of the second list, or stands
/// alone when that list is empty.
fn aligned_classes(first: Vec<String>, second: Vec<String>, align: &str) -> Vec<String> {
    let mut classes = first;
    if align.is_empty() {
        classes.extend(second);
    } else if second.is_empty() {
        classes.push(align.to_string());
    } else {
        let last = second.len() - 1;
        for (index, mut class) in second.into_iter().enumerate() {
            if index == last {
                class.push_str(align);
            }
            classes.push(class);
        }
    }
    classes
}

/// The `width`/`height` attributes of an image, each normalized and scaled by an `:scale:` option.
fn image_dimensions(options: &[(String, String)]) -> Vec<(String, String)> {
    let scale = options
        .iter()
        .find(|(k, _)| k == "scale")
        .and_then(|(_, v)| parse_scale(v));
    let mut attributes = Vec::new();
    for (key, value) in options {
        if key == "width" || key == "height" {
            attributes.push((key.clone(), normalize_dimension(value, scale)));
        }
    }
    attributes
}

/// A length with the unit categories the output distinguishes: integral pixels, a percentage, or a
/// value in some other unit.
enum Dimension {
    Pixel(f64),
    Percent(f64),
    Other(f64, String),
}

/// Parse a `:scale:` value into its factor and whether it was written as a percentage. A bare number
/// scales directly; a trailing `%` divides by a hundred.
fn parse_scale(value: &str) -> Option<(f64, bool)> {
    let value = value.trim();
    let percent = value.contains('%');
    let digits: String = value
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    digits.parse::<f64>().ok().map(|factor| (factor, percent))
}

/// Normalize a dimension and apply a scale factor: pixels round to the nearest integer (ties to
/// even), percentages always carry a fractional part, and other units keep their shortest form.
fn normalize_dimension(value: &str, scale: Option<(f64, bool)>) -> String {
    let Some(dimension) = parse_dimension(value) else {
        return value.to_string();
    };
    let dimension = scale_dimension(dimension, scale);
    match dimension {
        Dimension::Pixel(pixels) => format!("{}px", pixels.round_ties_even()),
        Dimension::Percent(percent) => {
            let text = format!("{percent}");
            if text.contains('.') {
                format!("{text}%")
            } else {
                format!("{text}.0%")
            }
        }
        Dimension::Other(magnitude, unit) => format!("{magnitude}{unit}"),
    }
}

fn parse_dimension(value: &str) -> Option<Dimension> {
    let value = value.trim();
    let split = value
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_digit() || *c == '.'))
        .map_or(value.len(), |(index, _)| index);
    let magnitude: f64 = value.get(..split)?.parse().ok()?;
    let unit = value.get(split..).unwrap_or("").trim();
    Some(match unit {
        "" | "px" => Dimension::Pixel(magnitude.trunc()),
        "%" => Dimension::Percent(magnitude),
        other => Dimension::Other(magnitude, other.to_string()),
    })
}

fn scale_dimension(dimension: Dimension, scale: Option<(f64, bool)>) -> Dimension {
    let Some((factor, percent)) = scale else {
        return dimension;
    };
    let divisor = if percent { 100.0 } else { 1.0 };
    let apply = |value: f64| value * factor / divisor;
    match dimension {
        Dimension::Pixel(value) => Dimension::Pixel(apply(value)),
        Dimension::Percent(value) => Dimension::Percent(apply(value)),
        Dimension::Other(value, unit) => Dimension::Other(apply(value), unit),
    }
}

pub(super) fn class_div(classes: Vec<String>, blocks: Vec<Block>) -> Block {
    Block::Div(
        Box::new(Attr {
            id: carta_ast::Text::default(),
            classes: classes.into_iter().map(Into::into).collect(),
            attributes: Vec::new(),
        }),
        blocks,
    )
}

/// When a paragraph's only content is an attribute-free span (the shape a multi-inline substitution
/// expands to), the span dissolves into the paragraph, which carries its inlines directly.
pub(super) fn splice_lone_span(mut inlines: Vec<Inline>) -> Vec<Inline> {
    let lone_plain_span = matches!(
        inlines.as_slice(),
        [Inline::Span(attr, _)]
            if attr.id.is_empty() && attr.classes.is_empty() && attr.attributes.is_empty()
    );
    if lone_plain_span && let Some(Inline::Span(_, inner)) = inlines.pop() {
        return inner;
    }
    inlines
}

/// Normalize the text of an inline literal: a line break within it folds to a single space, interior
/// spacing is otherwise preserved, and leading and trailing whitespace is removed.
pub(super) fn normalize_inline_literal(content: &str) -> String {
    content.replace('\n', " ").trim().to_string()
}

/// An attribute set carrying only classes, with no identifier or key-value attributes.
pub(super) fn class_attr(classes: Vec<String>) -> Attr {
    Attr {
        id: carta_ast::Text::default(),
        classes: classes.into_iter().map(Into::into).collect(),
        attributes: Vec::new(),
    }
}

/// Attach internal-target identifiers to the block they precede. A single target immediately before
/// a section heading supplies the heading's identifier; otherwise each target wraps the block in a
/// division carrying its identifier, the last target sitting innermost.
pub(super) fn attach_targets(mut blocks: Vec<Block>, mut targets: Vec<String>) -> Vec<Block> {
    // Targets before a section title attach to it: the last takes the title's identifier, the rest
    // become empty spans appended in reverse, each keeping its name for linking.
    if let [Block::Header(_, attr, inlines)] = blocks.as_mut_slice()
        && let Some(last) = targets.pop()
    {
        attr.id = last.into();
        for name in targets.into_iter().rev() {
            inlines.push(Inline::Span(
                Box::new(Attr {
                    id: name.into(),
                    classes: Vec::new(),
                    attributes: Vec::new(),
                }),
                Vec::new(),
            ));
        }
        return blocks;
    }
    for name in targets.into_iter().rev() {
        blocks = vec![Block::Div(
            Box::new(Attr {
                id: name.into(),
                classes: Vec::new(),
                attributes: Vec::new(),
            }),
            blocks,
        )];
    }
    blocks
}

/// Split a directive's options into the identifier it sets (`:name:`), the extra classes it adds
/// (`:class:`), and the remaining options carried as attributes, each in source order.
pub(super) fn common_options(
    options: &[(String, String)],
) -> (String, Vec<String>, Vec<(String, String)>) {
    let mut id = String::new();
    let mut classes = Vec::new();
    let mut attributes = Vec::new();
    for (key, value) in options {
        match key.as_str() {
            "name" => id.clone_from(value),
            "class" => classes.extend(value.split_whitespace().map(str::to_string)),
            other => attributes.push((other.to_string(), value.clone())),
        }
    }
    (id, classes, attributes)
}

/// Wrap a directive's blocks in a division named for the directive, folding its common options into
/// the division's identifier, classes, and attributes. The directive name leads the class list.
pub(super) fn options_div(name: &str, options: &[(String, String)], blocks: Vec<Block>) -> Block {
    let (id, extra, attributes) = common_options(options);
    let mut classes = vec![name.to_string()];
    classes.extend(extra);
    Block::Div(
        Box::new(Attr {
            id: id.into(),
            classes: classes.into_iter().map(Into::into).collect(),
            attributes: attributes
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        }),
        blocks,
    )
}

/// Group a directive body into the runs of consecutive non-blank lines, joined with newlines and
/// trimmed. A blank line separates one group from the next; empty groups are dropped.
pub(super) fn blank_separated(lines: &[String]) -> Vec<String> {
    let mut groups = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            if !current.is_empty() {
                groups.push(current.join("\n").trim().to_string());
                current.clear();
            }
        } else {
            current.push(line);
        }
    }
    if !current.is_empty() {
        groups.push(current.join("\n").trim().to_string());
    }
    groups
}

pub(super) fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Demote a leading paragraph to a plain block, leaving any other block unchanged.
pub(super) fn to_plain(block: Block) -> Block {
    match block {
        Block::Para(inlines) => Block::Plain(inlines),
        other => other,
    }
}
