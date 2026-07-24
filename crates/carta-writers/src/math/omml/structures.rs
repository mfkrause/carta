//! Fraction, radical, accent, fence, matrix, and grid lowering for the OMML math backend.

use super::super::parse::{Atom, BinomKind, Body, ColumnAlign, Delim, GridKind, MatrixDelim};
use super::{Element, Style, lower_seq, non_empty, run, script_slot, wrap};

pub(super) fn fraction(
    numerator: &[Atom],
    denominator: &[Atom],
    kind: &'static str,
    style: Style,
    depth: usize,
) -> Option<Element> {
    Some(
        Element::new("m:f")
            .child(Element::new("m:fPr").child(Element::new("m:type").attr("m:val", kind)))
            .child(wrap("m:num", script_slot(numerator, style, depth)?))
            .child(wrap("m:den", script_slot(denominator, style, depth)?)),
    )
}

pub(super) fn radical(
    index: Option<&[Atom]>,
    radicand: &[Atom],
    style: Style,
    depth: usize,
) -> Option<Element> {
    let body = wrap("m:e", script_slot(radicand, style, depth)?);
    Some(match index {
        None => Element::new("m:rad")
            .child(Element::new("m:radPr").child(Element::new("m:degHide").attr("m:val", "on")))
            .child(Element::new("m:deg"))
            .child(body),
        Some(index) => Element::new("m:rad")
            .child(wrap("m:deg", script_slot(index, style, depth)?))
            .child(body),
    })
}

pub(super) fn accent(name: &str, base: &[Atom], style: Style, depth: usize) -> Option<Element> {
    let inner = wrap("m:e", script_slot(base, style, depth)?);
    Some(match name {
        "overline" => over_bar("top", inner),
        "underline" => over_bar("bot", inner),
        _ => {
            let mark = accent_mark(name)?;
            Element::new("m:acc")
                .child(
                    Element::new("m:accPr")
                        .child(Element::new("m:chr").attr("m:val", mark.to_string())),
                )
                .child(inner)
        }
    })
}

fn over_bar(position: &'static str, inner: Element) -> Element {
    Element::new("m:bar")
        .child(Element::new("m:barPr").child(Element::new("m:pos").attr("m:val", position)))
        .child(inner)
}

/// The combining mark an accent command places over its base. `\overline`/`\underline` are handled
/// separately as bars; an unmapped accent reports the expression unconvertible.
fn accent_mark(name: &str) -> Option<char> {
    Some(match name {
        "bar" => '\u{203E}',
        "hat" | "widehat" => '\u{0302}',
        "tilde" | "widetilde" => '\u{0303}',
        "vec" | "overrightarrow" => '\u{20D7}',
        "overleftarrow" => '\u{20D6}',
        "dot" => '\u{0307}',
        "ddot" => '\u{0308}',
        "dddot" => '\u{20DB}',
        "ddddot" => '\u{20DC}',
        "check" => '\u{030C}',
        "breve" => '\u{0306}',
        "acute" => '\u{0301}',
        "grave" => '\u{0300}',
        "mathring" => '\u{030A}',
        "overleftrightarrow" => '\u{20E1}',
        "underleftarrow" => '\u{20EE}',
        "underrightarrow" => '\u{20EF}',
        _ => return None,
    })
}

pub(super) fn binomial(
    kind: BinomKind,
    top: &[Atom],
    bottom: &[Atom],
    style: Style,
    depth: usize,
) -> Option<Element> {
    let (open, close) = match kind {
        BinomKind::Paren => ("(", ")"),
        BinomKind::Brace => ("{", "}"),
        BinomKind::Brack => ("[", "]"),
    };
    let stack = fraction(top, bottom, "noBar", style, depth)?;
    Some(fence(open, close, vec![stack]))
}

/// A stretchable delimiter fence around some content. An empty fence keeps an empty content slot
/// rather than a filler, since the delimiters alone convey the grouping.
pub(super) fn fence(open: &str, close: &str, content: Vec<Element>) -> Element {
    Element::new("m:d")
        .child(
            Element::new("m:dPr")
                .child(Element::new("m:begChr").attr("m:val", open))
                .child(Element::new("m:sepChr").attr("m:val", ""))
                .child(Element::new("m:endChr").attr("m:val", close))
                .child(Element::new("m:grow")),
        )
        .child(wrap("m:e", content))
}

pub(super) fn delimited(
    open: Option<Delim>,
    close: Option<Delim>,
    content: &[Atom],
    style: Style,
    depth: usize,
) -> Option<Element> {
    let open = open.map_or("", |delimiter| delimiter_glyph(delimiter, true));
    let close = close.map_or("", |delimiter| delimiter_glyph(delimiter, false));

    // A `\middle` divider partitions the fence into consecutive slots separated by its glyph.
    let mut separator = "";
    let mut slots: Vec<Vec<Element>> = Vec::new();
    let mut start = 0usize;
    for (index, atom) in content.iter().enumerate() {
        if let Body::Middle(divider, open_side) = &atom.body {
            separator = divider.map_or("", |delimiter| delimiter_glyph(delimiter, *open_side));
            slots.push(lower_seq(
                content.get(start..index)?,
                style,
                depth + 1,
                false,
            )?);
            start = index + 1;
        }
    }
    if slots.is_empty() {
        return Some(fence(
            open,
            close,
            lower_seq(content, style, depth + 1, false)?,
        ));
    }
    slots.push(lower_seq(content.get(start..)?, style, depth + 1, false)?);

    let mut element = Element::new("m:d").child(
        Element::new("m:dPr")
            .child(Element::new("m:begChr").attr("m:val", open))
            .child(Element::new("m:sepChr").attr("m:val", separator))
            .child(Element::new("m:endChr").attr("m:val", close))
            .child(Element::new("m:grow")),
    );
    for slot in slots {
        element = element.child(wrap("m:e", non_empty(slot)));
    }
    Some(element)
}

/// The glyph a stretchable delimiter renders on its opening or closing side.
fn delimiter_glyph(delimiter: Delim, open: bool) -> &'static str {
    match delimiter {
        Delim::Paren => side(open, "(", ")"),
        Delim::Bracket => side(open, "[", "]"),
        Delim::Brace => side(open, "{", "}"),
        Delim::Bar => "|",
        Delim::BarVert => "\u{2225}",
        Delim::DoubleBar => "\u{2016}",
        Delim::Angle => side(open, "\u{27E8}", "\u{27E9}"),
        Delim::Floor => side(open, "\u{230A}", "\u{230B}"),
        Delim::Ceil => side(open, "\u{2308}", "\u{2309}"),
        Delim::CornerUpperLeft => "\u{231C}",
        Delim::CornerUpperRight => "\u{231D}",
    }
}

fn side(open: bool, opening: &'static str, closing: &'static str) -> &'static str {
    if open { opening } else { closing }
}

/// A boxed expression, framed on all four sides.
pub(super) fn border_box(argument: &[Atom], style: Style, depth: usize) -> Option<Element> {
    Some(Element::new("m:borderBox").child(wrap(
        "m:e",
        non_empty(lower_seq(argument, style, depth + 1, false)?),
    )))
}

pub(super) fn matrix(
    delimiter: MatrixDelim,
    rows: &[Vec<Vec<Atom>>],
    style: Style,
    depth: usize,
) -> Option<Element> {
    let grid = grid_body(rows, ColumnJustify::Center, style, depth)?;
    Some(match delimiter {
        MatrixDelim::None => grid,
        MatrixDelim::Paren => fence("(", ")", vec![grid]),
        MatrixDelim::Bracket => fence("[", "]", vec![grid]),
        MatrixDelim::Brace => fence("{", "}", vec![grid]),
        MatrixDelim::Bar => fence("\u{2223}", "\u{2223}", vec![grid]),
        MatrixDelim::DoubleBar => fence("\u{2225}", "\u{2225}", vec![grid]),
    })
}

/// The per-column horizontal justification of a matrix, indexed left to right.
#[derive(Clone, Copy)]
enum ColumnJustify<'a> {
    /// Every column centered.
    Center,
    /// Every column left-justified.
    Left,
    /// Every column right-justified.
    Right,
    /// Right, center, left, repeating: each successive alignment marker meets a column boundary.
    RightCenterLeft,
    /// Left, right, repeating: the two edges of a flush-both-sides layout.
    LeftRight,
    /// Each column set to the justification an array's column specification declares for it; columns
    /// past the end of the specification center.
    Explicit(&'a [ColumnAlign]),
}

impl ColumnJustify<'_> {
    fn at(self, column: usize) -> &'static str {
        match self {
            ColumnJustify::Center => "center",
            ColumnJustify::Left => "left",
            ColumnJustify::Right => "right",
            ColumnJustify::RightCenterLeft => match column % 3 {
                0 => "right",
                1 => "center",
                _ => "left",
            },
            ColumnJustify::LeftRight => {
                if column.is_multiple_of(2) {
                    "left"
                } else {
                    "right"
                }
            }
            ColumnJustify::Explicit(aligns) => match aligns.get(column) {
                Some(ColumnAlign::Left) => "left",
                Some(ColumnAlign::Right) => "right",
                _ => "center",
            },
        }
    }
}

/// Lower a grid environment. A case block is fenced with a left brace; a substack, an array, and the
/// centered stacking environments are centered matrices; an aligned block with alignment markers is
/// an equation array whose cells are joined by the marker, and one without is a right-justified
/// single-column matrix; the flush environments are matrices whose columns cycle through a
/// justification pattern so each alignment marker meets a column boundary.
pub(super) fn grid(
    kind: GridKind,
    aligns: &[ColumnAlign],
    rows: &[Vec<Vec<Atom>>],
    style: Style,
    depth: usize,
) -> Option<Vec<Element>> {
    Some(match kind {
        GridKind::Cases => {
            vec![fence(
                "{",
                "",
                vec![grid_body(rows, ColumnJustify::Left, style, depth)?],
            )]
        }
        GridKind::Array => {
            let justification = if aligns.is_empty() {
                ColumnJustify::Center
            } else {
                ColumnJustify::Explicit(aligns)
            };
            vec![grid_body(rows, justification, style, depth)?]
        }
        GridKind::Substack | GridKind::Gathered => {
            vec![grid_body(rows, ColumnJustify::Center, style, depth)?]
        }
        // With alignment markers cells join into equation-array lines; with none the block is a
        // right-justified single-column matrix.
        GridKind::Aligned => {
            let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
            if columns <= 1 {
                vec![grid_body(rows, ColumnJustify::Right, style, depth)?]
            } else {
                vec![equation_array(rows, style, depth)?]
            }
        }
        GridKind::Eqnarray => vec![grid_body(
            rows,
            ColumnJustify::RightCenterLeft,
            style,
            depth,
        )?],
        GridKind::Flalign => vec![grid_body(rows, ColumnJustify::LeftRight, style, depth)?],
    })
}

/// The `<m:m>` matrix body: column properties for the widest row, then a matrix row per source row.
fn grid_body(
    rows: &[Vec<Vec<Atom>>],
    justification: ColumnJustify<'_>,
    style: Style,
    depth: usize,
) -> Option<Element> {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut column_properties = Element::new("m:mcs");
    for column in 0..columns {
        column_properties = column_properties.child(
            Element::new("m:mc").child(
                Element::new("m:mcPr")
                    .child(Element::new("m:mcJc").attr("m:val", justification.at(column)))
                    .child(Element::new("m:count").attr("m:val", "1")),
            ),
        );
    }
    let mut grid = Element::new("m:m").child(
        Element::new("m:mPr")
            .child(Element::new("m:baseJc").attr("m:val", "center"))
            .child(Element::new("m:plcHide").attr("m:val", "on"))
            .child(column_properties),
    );
    for row in rows {
        let mut matrix_row = Element::new("m:mr");
        for cell in row {
            matrix_row = matrix_row.child(wrap("m:e", script_slot(cell, style, depth)?));
        }
        grid = grid.child(matrix_row);
    }
    Some(grid)
}

/// An aligned block as an equation array: each source row is one array line, its cells joined by the
/// literal alignment marker that separated them.
fn equation_array(rows: &[Vec<Vec<Atom>>], style: Style, depth: usize) -> Option<Element> {
    let mut array = Element::new("m:eqArr");
    for row in rows {
        let mut line = Vec::new();
        for (column, cell) in row.iter().enumerate() {
            if column > 0 {
                line.push(run("&", None));
            }
            line.append(&mut lower_seq(cell, style, depth + 1, false)?);
        }
        array = array.child(wrap("m:e", non_empty(line)));
    }
    Some(array)
}
