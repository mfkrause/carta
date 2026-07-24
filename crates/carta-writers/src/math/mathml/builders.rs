//! Construct builders for the MathML backend: tables, fences, sized delimiters, and the
//! remaining multi-part lowerings the nucleus dispatch calls.

use super::super::parse::{
    Atom, Body, BraceKind, ColumnAlign, Delim, GridKind, MatrixDelim, ModKind, StackSide,
};
use super::super::symbols::{self, Class};
use super::{
    Element, Node, apply_scripts, apply_sibling, char_glyph, leaf, lower_seq, nucleus, slot,
};

pub(super) fn binomial(top: &[Atom], bottom: &[Atom], display: bool, depth: usize) -> Node {
    let stack = Node::Element(
        Element::new("mfrac")
            .attr("linethickness", "0")
            .node(slot(top, display, depth))
            .node(slot(bottom, display, depth)),
    );
    fenced("(", ")", vec![stack])
}

pub(super) fn matrix(
    delimiter: MatrixDelim,
    rows: &[Vec<Vec<Atom>>],
    display: bool,
    depth: usize,
) -> Node {
    let table = table_element(
        &ColumnScheme::Uniform(ColumnAlign::Center),
        false,
        rows,
        display,
        depth,
    );
    let (open, close) = match delimiter {
        MatrixDelim::None => return table,
        MatrixDelim::Paren => ("(", ")"),
        MatrixDelim::Bracket => ("[", "]"),
        MatrixDelim::Brace => ("{", "}"),
        MatrixDelim::Bar => ("|", "|"),
        MatrixDelim::DoubleBar => ("\u{2016}", "\u{2016}"),
    };
    fenced(open, close, vec![table])
}

pub(super) fn grid(
    kind: GridKind,
    aligns: &[ColumnAlign],
    rows: &[Vec<Vec<Atom>>],
    display: bool,
    depth: usize,
) -> Node {
    match kind {
        GridKind::Array => {
            table_element(&ColumnScheme::Explicit(aligns), false, rows, display, depth)
        }
        GridKind::Aligned => table_element(&ColumnScheme::Aligned, false, rows, display, depth),
        GridKind::Eqnarray => table_element(&ColumnScheme::Eqnarray, false, rows, display, depth),
        GridKind::Flalign => table_element(&ColumnScheme::Flalign, false, rows, display, depth),
        GridKind::Gathered => table_element(
            &ColumnScheme::Uniform(ColumnAlign::Center),
            false,
            rows,
            display,
            depth,
        ),
        // A `\substack` stacks each row as a grouped sub-expression rather than a flat cell.
        GridKind::Substack => table_element(
            &ColumnScheme::Uniform(ColumnAlign::Center),
            true,
            rows,
            display,
            depth,
        ),
        // A `cases` block is fenced by a single left brace with no closing delimiter.
        GridKind::Cases => {
            let table = table_element(
                &ColumnScheme::Uniform(ColumnAlign::Left),
                false,
                rows,
                display,
                depth,
            );
            Node::Element(
                Element::new("mrow")
                    .node(fence_operator("{", true))
                    .node(table),
            )
        }
    }
}

/// How a tabular environment justifies its columns.
enum ColumnScheme<'a> {
    /// Every column takes one fixed alignment, with no inter-column padding.
    Uniform(ColumnAlign),
    /// Each column takes the alignment its position declares, with no inter-column padding.
    Explicit(&'a [ColumnAlign]),
    /// Columns alternate right, left; a multi-column block drops the gap between the pair so the two
    /// alignment markers meet.
    Aligned,
    /// Columns cycle right, center, left so each alignment marker meets a column boundary.
    Eqnarray,
    /// Columns cycle left, right for a flush-both-sides layout.
    Flalign,
}

impl ColumnScheme<'_> {
    /// The justification of the column at `index`.
    fn align(&self, index: usize) -> ColumnAlign {
        match self {
            ColumnScheme::Uniform(align) => *align,
            ColumnScheme::Explicit(aligns) => {
                aligns.get(index).copied().unwrap_or(ColumnAlign::Center)
            }
            ColumnScheme::Aligned => alternate(index, ColumnAlign::Right, ColumnAlign::Left),
            ColumnScheme::Eqnarray => match index % 3 {
                0 => ColumnAlign::Right,
                1 => ColumnAlign::Center,
                _ => ColumnAlign::Left,
            },
            ColumnScheme::Flalign => alternate(index, ColumnAlign::Left, ColumnAlign::Right),
        }
    }

    /// Whether the scheme collapses the gap between adjacent columns (only the alternating aligned
    /// layout, and only once it actually has more than one column).
    fn collapses_gap(&self) -> bool {
        matches!(self, ColumnScheme::Aligned)
    }
}

/// The first alignment on even columns, the second on odd columns.
fn alternate(index: usize, even: ColumnAlign, odd: ColumnAlign) -> ColumnAlign {
    if index.is_multiple_of(2) { even } else { odd }
}

/// Build an `<mtable>` from a grid of cells, tagging each cell with its column's justification. Cells
/// carry a flat run of nodes except in a grouped scheme, where each is collapsed to one node.
fn table_element(
    scheme: &ColumnScheme,
    group_cells: bool,
    rows: &[Vec<Vec<Atom>>],
    display: bool,
    depth: usize,
) -> Node {
    let multi_column = rows.iter().map(Vec::len).max().unwrap_or(0) > 1;
    let mut table = Element::new("mtable");
    for row in rows {
        let mut tr = Element::new("mtr");
        for (column, cell) in row.iter().enumerate() {
            let align = scheme.align(column);
            let (columnalign, style) =
                cell_attributes(align, scheme.collapses_gap() && multi_column);
            let mut td = Element::new("mtd")
                .attr("columnalign", columnalign)
                .attr("style", style);
            if group_cells {
                td = td.node(slot(cell, display, depth));
            } else {
                for node in lower_seq(cell, display, depth + 1) {
                    td = td.node(node);
                }
            }
            tr = tr.node(Node::Element(td));
        }
        table = table.node(Node::Element(tr));
    }
    Node::Element(table)
}

/// A cell's `columnalign` value and CSS `style`, dropping the trailing gap on the aligned side when the
/// layout collapses it.
fn cell_attributes(align: ColumnAlign, collapse: bool) -> (&'static str, String) {
    let dir = match align {
        ColumnAlign::Left => "left",
        ColumnAlign::Center => "center",
        ColumnAlign::Right => "right",
    };
    let style = match (collapse, align) {
        (true, ColumnAlign::Right) => format!("text-align: {dir}; padding-right: 0"),
        (true, ColumnAlign::Left) => format!("text-align: {dir}; padding-left: 0"),
        _ => format!("text-align: {dir}"),
    };
    (dir, style)
}

/// Lower a horizontal brace: the group under (or over) its brace glyph, with a matching-side label
/// (a superscript over an over-brace, a subscript under an under-brace) stacked as an outer limit.
pub(super) fn brace_atom(
    kind: BraceKind,
    inner: &[Atom],
    atom: &Atom,
    display: bool,
    depth: usize,
) -> Vec<Node> {
    let body = slot(inner, display, depth);
    let (wrapper, glyph) = match kind {
        BraceKind::Over => ("mover", "\u{23DE}"),
        BraceKind::Under => ("munder", "\u{23DF}"),
    };
    // The stretch accent rides on the brace glyph itself; the stacking element carries no accent.
    let brace_mark = Node::Element(Element::new("mo").attr("accent", "true").text(glyph));
    let core = Node::Element(Element::new(wrapper).node(body).node(brace_mark));

    // The matching-side script becomes the stacked label; any other applies as an ordinary script.
    let (label, remaining_subscript, remaining_superscript) = match kind {
        BraceKind::Over => (atom.sup.as_deref(), atom.sub.as_deref(), None),
        BraceKind::Under => (atom.sub.as_deref(), None, atom.sup.as_deref()),
    };
    let mut node = core;
    if let Some(label) = label {
        let stacked = match kind {
            BraceKind::Over => "mover",
            BraceKind::Under => "munder",
        };
        node = Node::Element(
            Element::new(stacked)
                .node(node)
                .node(slot(label, display, depth)),
        );
    }
    if remaining_subscript.is_some() || remaining_superscript.is_some() || !atom.siblings.is_empty()
    {
        node = apply_scripts(
            node,
            remaining_subscript,
            remaining_superscript,
            false,
            display,
            depth,
        );
        for sibling in &atom.siblings {
            node = apply_sibling(node, sibling, false, display, depth);
        }
    }
    vec![node]
}

pub(super) fn stack_over_under(
    side: StackSide,
    mark: &[Atom],
    base: &[Atom],
    display: bool,
    depth: usize,
) -> Node {
    let wrapper = match side {
        StackSide::Over => "mover",
        StackSide::Under => "munder",
    };
    Node::Element(
        Element::new(wrapper)
            .node(slot(base, display, depth))
            .node(slot(mark, display, depth)),
    )
}

pub(super) fn ext_arrow(
    arrow: &str,
    below: Option<&[Atom]>,
    above: &[Atom],
    display: bool,
    depth: usize,
) -> Node {
    let glyph = match arrow {
        "arrow.l" => "\u{2190}",
        _ => "\u{2192}",
    };
    let arrow_mark = leaf("mo", glyph);
    match below {
        None => Node::Element(
            Element::new("mover")
                .node(arrow_mark)
                .node(slot(above, display, depth)),
        ),
        Some(below) => Node::Element(
            Element::new("munderover")
                .node(arrow_mark)
                .node(slot(below, display, depth))
                .node(slot(above, display, depth)),
        ),
    }
}

/// A stretchable delimiter fence around some content: an opening `<mo>`, the content, and a closing
/// `<mo>`, all inside an `<mrow>`. An empty delimiter contributes no glyph on its side.
fn fenced(open: &str, close: &str, content: Vec<Node>) -> Node {
    let mut row = Element::new("mrow");
    if !open.is_empty() {
        row = row.node(fence_operator(open, true));
    }
    for node in content {
        row = row.node(node);
    }
    if !close.is_empty() {
        row = row.node(fence_operator(close, false));
    }
    Node::Element(row)
}

fn fence_operator(glyph: &str, opening: bool) -> Node {
    Node::Element(
        Element::new("mo")
            .attr("stretchy", "true")
            .attr("form", if opening { "prefix" } else { "postfix" })
            .text(glyph),
    )
}

pub(super) fn delimited(
    open: Option<Delim>,
    close: Option<Delim>,
    content: &[Atom],
    display: bool,
    depth: usize,
) -> Node {
    let open_glyph = open.map_or(String::new(), |delimiter| delimiter_glyph(delimiter, true));
    let close_glyph = close.map_or(String::new(), |delimiter| delimiter_glyph(delimiter, false));
    fenced(
        &open_glyph,
        &close_glyph,
        lower_seq(content, display, depth + 1),
    )
}

pub(super) fn big_delimiter(scale: u16, inner: &Atom, display: bool, depth: usize) -> Vec<Node> {
    let size = format!("{scale}%");
    let form = big_form(&inner.body);
    let mut nodes = nucleus(&inner.body, display, depth);
    for node in &mut nodes {
        if let Node::Element(element) = node
            && (element.name == "mo" || element.name == "mi")
        {
            element.attributes.push(("minsize", size.clone()));
            element.attributes.push(("maxsize", size.clone()));
            element.attributes.push(("stretchy", "true".to_string()));
            if element.name == "mo"
                && let Some(form) = form
            {
                element.attributes.push(("form", form.to_string()));
            }
        }
    }
    nodes
}

/// The fence side a sized delimiter takes, from the glyph's math class: an opening delimiter is a
/// `prefix` operator, a closing one a `postfix` operator, anything else unsided.
fn big_form(body: &Body) -> Option<&'static str> {
    let class = match body {
        Body::Char(c) => char_glyph(*c).1,
        Body::Command(name) => symbols::symbol(name)?.class,
        _ => return None,
    };
    match class {
        Class::Open => Some("prefix"),
        Class::Close => Some("postfix"),
        _ => None,
    }
}

/// The glyph a stretchy delimiter renders on its opening or closing side.
pub(super) fn delimiter_glyph(delimiter: Delim, opening: bool) -> String {
    let glyph = match delimiter {
        Delim::Paren => {
            if opening {
                "("
            } else {
                ")"
            }
        }
        Delim::Bracket => {
            if opening {
                "["
            } else {
                "]"
            }
        }
        Delim::Brace => {
            if opening {
                "{"
            } else {
                "}"
            }
        }
        Delim::Bar => "|",
        Delim::BarVert => "\u{2225}",
        Delim::DoubleBar => "\u{2016}",
        Delim::Angle => {
            if opening {
                "\u{27E8}"
            } else {
                "\u{27E9}"
            }
        }
        Delim::Floor => {
            if opening {
                "\u{230A}"
            } else {
                "\u{230B}"
            }
        }
        Delim::Ceil => {
            if opening {
                "\u{2308}"
            } else {
                "\u{2309}"
            }
        }
        Delim::CornerUpperLeft => "\u{231C}",
        Delim::CornerUpperRight => "\u{231D}",
    };
    glyph.to_string()
}

/// Lower a `\not`-negated base: a precomposed negated relation, a combining long solidus over a
/// letter or relation, or the literal name when the base carries no meaningful strike.
pub(super) fn negated(base: &str) -> Node {
    if let Some(glyph) = symbols::negated_relation(base) {
        return leaf("mo", glyph.to_string());
    }
    match super::super::inlines::negated_base(base) {
        Some(super::super::inlines::NegatedBase::Relation(mut glyph)) => {
            glyph.push('\u{0338}');
            leaf("mo", glyph)
        }
        Some(super::super::inlines::NegatedBase::Italic(mut glyph)) => {
            glyph.push('\u{0338}');
            leaf("mi", glyph)
        }
        Some(super::super::inlines::NegatedBase::Upright(mut glyph)) => {
            glyph.push('\u{0338}');
            leaf("mn", glyph)
        }
        None => leaf("mi", base.to_string()),
    }
}

pub(super) fn negated_group(atoms: &[Atom], display: bool, depth: usize) -> Node {
    Node::Element(
        Element::new("mover")
            .attr("accent", "true")
            .node(slot(atoms, display, depth))
            .node(Node::Element(
                Element::new("mo").attr("accent", "true").text("\u{0338}"),
            )),
    )
}

/// Lower a modulo operator to its node sequence: a leading space, an optional opening parenthesis,
/// the `mod` word with a function-application marker, a following space, the bracketed modulus for the
/// parenthesised forms, and a closing parenthesis.
pub(super) fn modulo(
    kind: ModKind,
    argument: Option<&[Atom]>,
    display: bool,
    depth: usize,
) -> Vec<Node> {
    let lead = match kind {
        ModKind::Mod => "0.444em",
        _ => "0.222em",
    };
    let parenthesised = matches!(kind, ModKind::Pmod | ModKind::Pod);
    let mut inner = Vec::new();
    inner.push(Node::Element(Element::new("mspace").attr("width", lead)));
    if parenthesised {
        inner.push(fence_operator("(", true));
    }
    if !matches!(kind, ModKind::Pod) {
        let word = Node::Element(
            Element::new("mrow")
                .node(Node::Element(
                    Element::new("mi").attr("mathvariant", "normal").text("mod"),
                ))
                .node(leaf("mo", "\u{2061}")),
        );
        inner.push(word);
        inner.push(Node::Element(
            Element::new("mspace").attr("width", "0.222em"),
        ));
    }
    if let Some(argument) = argument {
        inner.append(&mut lower_seq(argument, display, depth + 1));
    }
    if parenthesised {
        inner.push(fence_operator(")", false));
    }
    let mut row = Element::new("mrow");
    row.children = inner;
    vec![Node::Element(row)]
}
