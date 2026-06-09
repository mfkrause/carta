//! Table geometry: column widths, alignments, spans and the leading header-column count, derived
//! from a `<table>`'s row and column structure.

use carta_ast::{Alignment, Attr, ColWidth, Row};

use super::tree::{Element, Node, attr_value, style_property};

pub(super) fn span_attr(cell: &Element, key: &str) -> i32 {
    attr_value(cell, key)
        .and_then(|v| v.trim().parse::<i32>().ok())
        .filter(|&n| n >= 1)
        .unwrap_or(1)
}

pub(super) fn cell_alignment(cell: &Element) -> Alignment {
    if let Some(align) = attr_value(cell, "align")
        && let Some(parsed) = parse_alignment(&align)
    {
        return parsed;
    }
    if let Some(style) = attr_value(cell, "style")
        && let Some(value) = style_property(&style, "text-align")
        && let Some(parsed) = parse_alignment(&value)
    {
        return parsed;
    }
    Alignment::AlignDefault
}

/// Drop the `text-align` declaration from a cell's `style` attribute (its alignment is held
/// separately) and re-render the surviving declarations as `prop: value` joined by `; `. When no
/// declaration remains, the `style` attribute is removed entirely.
pub(super) fn normalize_cell_style(attr: &mut Attr) {
    let Some(index) = attr.attributes.iter().position(|(key, _)| key == "style") else {
        return;
    };
    let Some((_, raw)) = attr.attributes.get(index) else {
        return;
    };
    let declarations: Vec<String> = raw
        .split(';')
        .filter_map(|decl| {
            let (key, value) = decl.split_once(':')?;
            let key = key.trim();
            if key.eq_ignore_ascii_case("text-align") || key.is_empty() {
                return None;
            }
            Some(format!("{key}: {}", value.trim()))
        })
        .collect();
    if declarations.is_empty() {
        attr.attributes.remove(index);
    } else if let Some(entry) = attr.attributes.get_mut(index) {
        entry.1 = declarations.join("; ");
    }
}

fn parse_alignment(value: &str) -> Option<Alignment> {
    match value.trim().to_ascii_lowercase().as_str() {
        "left" => Some(Alignment::AlignLeft),
        "right" => Some(Alignment::AlignRight),
        "center" => Some(Alignment::AlignCenter),
        _ => None,
    }
}

/// The width declared on a single `<col>` before star columns are resolved against the remainder.
enum DeclaredWidth {
    Fraction(f64),
    Star,
    None,
}

pub(super) fn column_widths(colgroup: &Element) -> Vec<ColWidth> {
    let mut declared = Vec::new();
    for child in &colgroup.children {
        let Node::Element(col) = child else { continue };
        if col.name != "col" {
            continue;
        }
        let span = span_attr(col, "span");
        let width = declared_width(col);
        for _ in 0..span.max(1) {
            declared.push(match &width {
                DeclaredWidth::Fraction(fraction) => DeclaredWidth::Fraction(*fraction),
                DeclaredWidth::Star => DeclaredWidth::Star,
                DeclaredWidth::None => DeclaredWidth::None,
            });
        }
    }
    resolve_star_widths(&declared)
}

/// The width of one `<col>`: a `width="*"` shares the remaining space, a `width="N%"` or
/// `style="width: N%"` is an explicit fraction, and anything else leaves the column unsized.
fn declared_width(col: &Element) -> DeclaredWidth {
    if let Some(value) = attr_value(col, "width") {
        let value = value.trim();
        if value == "*" {
            return DeclaredWidth::Star;
        }
        if let Some(fraction) = parse_percent(value) {
            return DeclaredWidth::Fraction(fraction);
        }
    }
    attr_value(col, "style")
        .and_then(|style| style_property(&style, "width"))
        .and_then(|value| parse_percent(&value))
        .map_or(DeclaredWidth::None, DeclaredWidth::Fraction)
}

fn parse_percent(value: &str) -> Option<f64> {
    value
        .trim()
        .strip_suffix('%')
        .and_then(|n| n.trim().parse::<f64>().ok())
        .map(|percent| percent / 100.0)
}

/// Turn declared widths into [`ColWidth`]s, distributing the space left over by explicit fractions
/// equally across the `width="*"` columns.
#[allow(clippy::cast_precision_loss)]
fn resolve_star_widths(declared: &[DeclaredWidth]) -> Vec<ColWidth> {
    let explicit: f64 = declared
        .iter()
        .filter_map(|width| match width {
            DeclaredWidth::Fraction(fraction) => Some(*fraction),
            _ => None,
        })
        .sum();
    let star_count = declared
        .iter()
        .filter(|width| matches!(width, DeclaredWidth::Star))
        .count();
    let star_width = if star_count == 0 {
        0.0
    } else {
        ((1.0 - explicit) / star_count as f64).max(0.0)
    };
    declared
        .iter()
        .map(|width| match width {
            DeclaredWidth::Fraction(fraction) => ColWidth::ColWidth(*fraction),
            DeclaredWidth::Star => ColWidth::ColWidth(star_width),
            DeclaredWidth::None => ColWidth::ColWidthDefault,
        })
        .collect()
}

pub(super) fn row_elements(section: &Element) -> Vec<&Element> {
    section
        .children
        .iter()
        .filter_map(|node| match node {
            Node::Element(tr) if tr.name == "tr" => Some(tr),
            _ => None,
        })
        .collect()
}

/// The count of leading header columns shared by every body row, used as a body's
/// `RowHeadColumns`. Each row's leading header span is measured over the expanded grid so a header
/// cell's `colspan` and a `rowspan` carried down from an earlier row both count. The result is that
/// shared count only when all rows agree; any disagreement yields zero.
pub(super) fn row_head_columns(rows: &[&Element]) -> i32 {
    let mut carried: Vec<Carry> = Vec::new();
    let mut counts = Vec::new();
    for tr in rows {
        let mut column = 0usize;
        let mut leading = true;
        let mut leading_count = 0usize;
        for node in &tr.children {
            while let Some(carry) = carried.get_mut(column).filter(|carry| carry.rows > 0) {
                carry.rows -= 1;
                if carry.header {
                    if leading {
                        leading_count += 1;
                    }
                } else {
                    leading = false;
                }
                column += 1;
            }
            let Node::Element(cell) = node else { continue };
            if cell.name != "td" && cell.name != "th" {
                continue;
            }
            let header = cell.name == "th";
            let col_span = usize::try_from(span_attr(cell, "colspan").max(1)).unwrap_or(1);
            let row_span = span_attr(cell, "rowspan").max(1);
            for _ in 0..col_span {
                if header {
                    if leading {
                        leading_count += 1;
                    }
                } else {
                    leading = false;
                }
                if row_span > 1 {
                    if carried.len() <= column {
                        carried.resize(column + 1, Carry::default());
                    }
                    if let Some(slot) = carried.get_mut(column) {
                        slot.rows = usize::try_from(row_span - 1).unwrap_or(0);
                        slot.header = header;
                    }
                }
                column += 1;
            }
        }
        counts.push(leading_count);
    }
    match counts.split_first() {
        Some((first, rest)) if rest.iter().all(|count| count == first) => {
            i32::try_from(*first).unwrap_or(0)
        }
        _ => 0,
    }
}

/// A cell spanning into rows below tracks how many more rows it occupies and whether it is a header.
#[derive(Default, Clone, Copy)]
struct Carry {
    rows: usize,
    header: bool,
}

fn row_width(row: &Row) -> usize {
    row.cells
        .iter()
        .map(|cell| usize::try_from(cell.col_span.max(1)).unwrap_or(1))
        .sum()
}

pub(super) fn table_width(
    head: &[Row],
    body: &[Row],
    foot: &[Row],
    colgroup_width: usize,
) -> usize {
    head.iter()
        .chain(body)
        .chain(foot)
        .map(row_width)
        .max()
        .unwrap_or(0)
        .max(colgroup_width)
}

pub(super) fn column_alignments(row: Option<&Row>, columns: usize) -> Vec<Alignment> {
    let mut aligns = vec![Alignment::AlignDefault; columns];
    let Some(row) = row else { return aligns };
    let mut index = 0;
    for cell in &row.cells {
        for _ in 0..cell.col_span.max(1) {
            if let Some(slot) = aligns.get_mut(index) {
                *slot = cell.align.clone();
            }
            index += 1;
        }
    }
    aligns
}
