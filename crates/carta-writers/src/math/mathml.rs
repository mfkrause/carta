//! Presentation MathML backend: lowers the shared math parse tree ([`super::parse`]) to the
//! MathML grammar an `OpenDocument` formula object carries.
//!
//! The tree is the same one the other backends consume; here it is walked into a small element tree
//! and serialized. A single-element sequence renders bare, a longer one inside an `<mrow>`, which
//! keeps grouping minimal the way MathML expects. The walk is total and panic-free — every construct
//! renders to some valid MathML, falling back to a text node for input with no structural form — and
//! is bounded against pathological nesting by an explicit depth limit.

use super::escape::{escape_attribute, escape_text};
use super::parse::{
    self, Atom, Body, BraceKind, ColumnAlign, Delim, FracStyle, GridKind, MatrixDelim, ModKind,
    ScriptKind, Sibling, StackSide, TextPiece,
};
use super::symbols::{self, Class};

/// Maximum structural nesting depth before the walk stops descending, rendering the offending
/// sub-expression as an empty group. The parser already bounds brace nesting well below this.
const MAX_DEPTH: usize = 256;

/// Convert TeX math source to a Presentation MathML `<math>` element: `display="inline"` for inline
/// math, `display="block"` for display math. Returns `None` only when the source cannot be parsed.
pub(crate) fn to_mathml(tex: &str, display: bool) -> Option<String> {
    let atoms = parse::parse(tex)?;
    let body = lower_seq(&atoms, display, 0);
    let mut root = Element::new("math")
        .attr("xmlns", "http://www.w3.org/1998/Math/MathML")
        .attr("display", if display { "block" } else { "inline" });
    // The whole expression forms one grouped row: a single element renders bare, a longer sequence
    // inside one `<mrow>`, an empty expression as no content at all.
    root.children = if body.is_empty() {
        Vec::new()
    } else {
        vec![group(body)]
    };
    let mut out = String::new();
    root.render(&mut out);
    Some(out)
}

// ----------------------------------------------------------------------------
// Minimal XML element tree
// ----------------------------------------------------------------------------

/// An XML element node: a tag, its ordered attributes, and its ordered children.
struct Element {
    name: &'static str,
    attributes: Vec<(&'static str, String)>,
    children: Vec<Node>,
}

enum Node {
    Element(Element),
    Text(String),
}

impl Element {
    fn new(name: &'static str) -> Self {
        Element {
            name,
            attributes: Vec::new(),
            children: Vec::new(),
        }
    }

    fn attr(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.attributes.push((name, value.into()));
        self
    }

    fn text(mut self, text: impl Into<String>) -> Self {
        self.children.push(Node::Text(text.into()));
        self
    }

    fn node(mut self, child: Node) -> Self {
        self.children.push(child);
        self
    }

    fn render(&self, out: &mut String) {
        out.push('<');
        out.push_str(self.name);
        for (key, value) in &self.attributes {
            out.push(' ');
            out.push_str(key);
            out.push_str("=\"");
            escape_attribute(value, out);
            out.push('"');
        }
        out.push('>');
        for child in &self.children {
            match child {
                Node::Element(element) => element.render(out),
                Node::Text(text) => escape_text(text, out),
            }
        }
        out.push_str("</");
        out.push_str(self.name);
        out.push('>');
    }
}

/// A leaf element (`<mi>`, `<mo>`, …) carrying one text glyph.
fn leaf(tag: &'static str, text: impl Into<String>) -> Node {
    Node::Element(Element::new(tag).text(text))
}

// ----------------------------------------------------------------------------
// Sequence and grouping
// ----------------------------------------------------------------------------

/// Lower a run of atoms to a run of nodes, flattening each atom's own nodes into one sequence.
fn lower_seq(atoms: &[Atom], display: bool, depth: usize) -> Vec<Node> {
    if depth > MAX_DEPTH {
        return Vec::new();
    }
    let mut out = Vec::new();
    for atom in atoms {
        out.append(&mut lower_atom(atom, display, depth));
    }
    out
}

/// Collapse a run of nodes to a single node: one node bare, several wrapped in an `<mrow>`, none an
/// empty `<mrow>` so a required slot still has content.
fn group(mut nodes: Vec<Node>) -> Node {
    if nodes.len() == 1 {
        if let Some(node) = nodes.pop() {
            return node;
        }
    }
    let mut row = Element::new("mrow");
    row.children = nodes;
    Node::Element(row)
}

/// Lower a slot (a script argument, a fraction part, a cell) to a single grouped node.
fn slot(atoms: &[Atom], display: bool, depth: usize) -> Node {
    group(lower_seq(atoms, display, depth + 1))
}

// ----------------------------------------------------------------------------
// Atom lowering
// ----------------------------------------------------------------------------

/// Lower one atom: its nucleus wrapped by whatever script chain it carries.
fn lower_atom(atom: &Atom, display: bool, depth: usize) -> Vec<Node> {
    // A horizontal brace turns its matching-side label into a stacked limit rather than an ordinary
    // script, so it is resolved before the generic script pass sees the atom.
    if let Body::Brace(kind, inner) = &atom.body {
        return brace_atom(*kind, inner, atom, display, depth);
    }

    let base = group(nucleus(&atom.body, display, depth));
    if atom.sub.is_none() && atom.sup.is_none() && atom.siblings.is_empty() {
        return vec![base];
    }

    let stack = stacks(atom, display);
    let mut node = apply_scripts(
        base,
        atom.sub.as_deref(),
        atom.sup.as_deref(),
        stack,
        display,
        depth,
    );
    for sibling in &atom.siblings {
        node = apply_sibling(node, sibling, stack, display, depth);
    }
    vec![node]
}

/// Whether an atom sets its scripts as stacked limits (under/over) rather than beside it: an explicit
/// `\limits`/`\nolimits`, otherwise a display-mode limit operator (`\sum`, `\lim`, a large
/// set/logic operator). Integral-style operators keep their scripts beside them.
fn stacks(atom: &Atom, display: bool) -> bool {
    atom.limits.unwrap_or(display && is_under_over(&atom.body))
}

fn is_under_over(body: &Body) -> bool {
    match body {
        Body::Command(name) => {
            matches!(
                name.as_str(),
                "sum"
                    | "prod"
                    | "coprod"
                    | "bigcup"
                    | "bigcap"
                    | "bigvee"
                    | "bigwedge"
                    | "bigsqcup"
            ) || symbols::named_function(name).is_some_and(|(_, limits)| limits)
        }
        Body::Char(c) => matches!(
            c,
            '\u{2211}'
                | '\u{220F}'
                | '\u{2210}'
                | '\u{22C3}'
                | '\u{22C2}'
                | '\u{22C1}'
                | '\u{22C0}'
                | '\u{2A06}'
        ),
        _ => false,
    }
}

/// Wrap a base node in a subscript, superscript, or both — beside the base (`msub`/`msup`/`msubsup`)
/// or stacked under and over it (`munder`/`mover`/`munderover`).
fn apply_scripts(
    base: Node,
    sub: Option<&[Atom]>,
    sup: Option<&[Atom]>,
    stack: bool,
    display: bool,
    depth: usize,
) -> Node {
    match (sub, sup) {
        (None, None) => base,
        (Some(sub), None) => Node::Element(
            Element::new(if stack { "munder" } else { "msub" })
                .node(base)
                .node(slot(sub, display, depth)),
        ),
        (None, Some(sup)) => Node::Element(
            Element::new(if stack { "mover" } else { "msup" })
                .node(base)
                .node(slot(sup, display, depth)),
        ),
        (Some(sub), Some(sup)) => Node::Element(
            Element::new(if stack { "munderover" } else { "msubsup" })
                .node(base)
                .node(slot(sub, display, depth))
                .node(slot(sup, display, depth)),
        ),
    }
}

fn apply_sibling(base: Node, sibling: &Sibling, stack: bool, display: bool, depth: usize) -> Node {
    match sibling.kind {
        ScriptKind::Sub => apply_scripts(base, Some(&sibling.atoms), None, stack, display, depth),
        ScriptKind::Sup => apply_scripts(base, None, Some(&sibling.atoms), stack, display, depth),
    }
}

// ----------------------------------------------------------------------------
// Nucleus lowering
// ----------------------------------------------------------------------------

/// Lower an atom's nucleus (its body without scripts) to zero or more nodes.
fn nucleus(body: &Body, display: bool, depth: usize) -> Vec<Node> {
    match body {
        Body::Char(c) => vec![char_leaf(*c)],
        Body::Number(digits) => vec![leaf("mn", digits.clone())],
        Body::ColonEq => vec![leaf("mo", ":=")],
        Body::Empty | Body::EmptyGroup | Body::Label(_) => Vec::new(),
        Body::Prime(count) => vec![leaf("mo", prime_marks(*count))],
        Body::Command(name) => command_nucleus(name),
        Body::Group(atoms) => vec![group(lower_seq(atoms, display, depth + 1))],
        Body::Styled(name, argument) => vec![styled(name, argument, display, depth)],
        Body::Frac(style, numerator, denominator) => {
            vec![fraction(*style, numerator, denominator, display, depth)]
        }
        Body::Sqrt(index, radicand) => vec![radical(index.as_deref(), radicand, display, depth)],
        Body::Accent(name, base) => vec![accent(name, base, display, depth)],
        Body::Text(name, pieces) => vec![text(name, pieces)],
        Body::Binom(_, top, bottom) => vec![binomial(top, bottom, display, depth)],
        Body::Matrix(delimiter, rows) => vec![matrix(*delimiter, rows, display, depth)],
        Body::Grid(kind, aligns, rows) => vec![grid(*kind, aligns, rows, display, depth)],
        Body::Delimited(open, close, content) => {
            vec![delimited(*open, *close, content, display, depth)]
        }
        Body::Middle(divider, open_side) => match divider {
            Some(delimiter) => vec![leaf("mo", delimiter_glyph(*delimiter, *open_side))],
            None => Vec::new(),
        },
        Body::Big(scale, inner) => big_delimiter(*scale, inner, display, depth),
        Body::Mod(kind, argument) => modulo(*kind, argument.as_deref(), display, depth),
        Body::Negated(base) => vec![negated(base)],
        Body::NegatedGroup(atoms) => vec![negated_group(atoms, display, depth)],
        Body::Stack(side, mark, base) => vec![stack_over_under(*side, mark, base, display, depth)],
        Body::ExtArrow(arrow, below, above) => {
            vec![ext_arrow(arrow, below.as_deref(), above, display, depth)]
        }
        // A brace is resolved before the generic nucleus pass, so it never reaches here.
        Body::Brace(_, group) => vec![self::group(lower_seq(group, display, depth + 1))],
    }
}

/// The leaf element for a single source character: a digit as `<mn>`, a letter as `<mi>`, and every
/// operator, relation, delimiter, or punctuation glyph as `<mo>`.
fn char_leaf(c: char) -> Node {
    if c.is_ascii_digit() {
        return leaf("mn", c.to_string());
    }
    if c.is_alphabetic() {
        return leaf("mi", c.to_string());
    }
    let (glyph, class) = char_glyph(c);
    match class {
        Class::Ord => leaf("mi", glyph),
        _ => leaf("mo", glyph),
    }
}

/// A single source character's glyph text and math class. A hyphen-minus prints as the minus sign.
fn char_glyph(c: char) -> (String, Class) {
    match c {
        '-' => ("\u{2212}".to_string(), Class::Bin),
        '+' | '*' => (c.to_string(), Class::Bin),
        '=' | '<' | '>' | ':' => (c.to_string(), Class::Rel),
        ',' | ';' => (c.to_string(), Class::Punct),
        '(' | '[' => (c.to_string(), Class::Open),
        ')' | ']' | '|' => (c.to_string(), Class::Close),
        _ => (c.to_string(), Class::Ord),
    }
}

/// Lower a control-sequence nucleus: an inter-atom spacing, a Greek letter, a symbol, or a named
/// operator. An unknown command renders as its literal name in an `<mi>`.
fn command_nucleus(name: &str) -> Vec<Node> {
    if let Some(width) = spacing_width(name) {
        return vec![Node::Element(Element::new("mspace").attr("width", width))];
    }
    if let Some((glyph, _)) = symbols::greek(name) {
        return vec![leaf("mi", glyph.to_string())];
    }
    if let Some(symbol) = symbols::symbol(name) {
        let tag = if symbol.italic || symbol.class == Class::Ord {
            "mi"
        } else {
            "mo"
        };
        return vec![leaf(tag, symbol.text.to_string())];
    }
    if let Some((word, _)) = symbols::named_function(name) {
        return vec![leaf("mi", word.to_string())];
    }
    vec![leaf("mi", name.to_string())]
}

/// The em-width a math spacing command sets, as MathML records widths. Unknown spacings contribute no
/// width and fold away.
fn spacing_width(name: &str) -> Option<&'static str> {
    Some(match name {
        "," | "thinspace" => "0.167em",
        ":" | ">" | "medspace" => "0.222em",
        ";" | "thickspace" => "0.278em",
        "!" | "negthinspace" => "-0.167em",
        "enspace" => "0.5em",
        "quad" => "1em",
        "qquad" => "2em",
        _ => return None,
    })
}

/// The precomposed prime run for `count` marks, repeating the single prime past the precomposed set.
fn prime_marks(count: u8) -> String {
    match count {
        1 => "\u{2032}".to_string(),
        2 => "\u{2033}".to_string(),
        3 => "\u{2034}".to_string(),
        4 => "\u{2057}".to_string(),
        other => "\u{2032}".repeat(usize::from(other)),
    }
}

// ----------------------------------------------------------------------------
// Two-dimensional structures
// ----------------------------------------------------------------------------

fn fraction(
    style: FracStyle,
    numerator: &[Atom],
    denominator: &[Atom],
    display: bool,
    depth: usize,
) -> Node {
    let mut frac = Element::new("mfrac");
    if style == FracStyle::Linear {
        frac = frac.attr("linethickness", "0");
    }
    Node::Element(frac.node(slot(numerator, display, depth)).node(slot(
        denominator,
        display,
        depth,
    )))
}

fn radical(index: Option<&[Atom]>, radicand: &[Atom], display: bool, depth: usize) -> Node {
    match index {
        None => Node::Element(Element::new("msqrt").node(slot(radicand, display, depth))),
        Some(index) => Node::Element(
            Element::new("mroot")
                .node(slot(radicand, display, depth))
                .node(slot(index, display, depth)),
        ),
    }
}

fn accent(name: &str, base: &[Atom], display: bool, depth: usize) -> Node {
    let inner = slot(base, display, depth);
    if name == "underline" {
        return Node::Element(
            Element::new("munder")
                .node(inner)
                .node(leaf("mo", "\u{0332}"))
                .attr("accentunder", "true"),
        );
    }
    let mark = accent_mark(name);
    Node::Element(
        Element::new("mover")
            .attr("accent", "true")
            .node(inner)
            .node(Node::Element(
                Element::new("mo").attr("accent", "true").text(mark),
            )),
    )
}

/// The combining mark an accent command places over its base, defaulting to a macron for an unmapped
/// accent so the construct still renders.
fn accent_mark(name: &str) -> String {
    let mark = match name {
        "hat" | "widehat" => '\u{0302}',
        "tilde" | "widetilde" => '\u{0303}',
        "vec" | "overrightarrow" => '\u{20D7}',
        "overleftarrow" => '\u{20D6}',
        "dot" => '\u{0307}',
        "ddot" => '\u{0308}',
        "dddot" => '\u{20DB}',
        "check" => '\u{030C}',
        "breve" => '\u{0306}',
        "acute" => '\u{0301}',
        "grave" => '\u{0300}',
        "mathring" => '\u{030A}',
        "bar" | "overline" => '\u{203E}',
        _ => '\u{203E}',
    };
    mark.to_string()
}

fn styled(name: &str, argument: &[Atom], display: bool, depth: usize) -> Node {
    if let Some(notation) = cancel_notation(name) {
        return Node::Element(
            Element::new("menclose")
                .attr("notation", notation)
                .node(slot(argument, display, depth)),
        );
    }
    if name == "boxed" {
        return Node::Element(
            Element::new("menclose")
                .attr("notation", "box")
                .node(slot(argument, display, depth)),
        );
    }
    let mut node = slot(argument, display, depth);
    if let Some(style) = math_style(name) {
        apply_math_style(&mut node, style);
    }
    node
}

/// The `menclose` notation a cancel-family command draws, or `None` for an ordinary styled wrapper.
fn cancel_notation(name: &str) -> Option<&'static str> {
    Some(match name {
        "cancel" => "updiagonalstrike",
        "bcancel" => "downdiagonalstrike",
        "xcancel" => "updiagonalstrike downdiagonalstrike",
        _ => return None,
    })
}

/// A font-variant alphabet a `\math…` command applies to its argument's identifier and number leaves.
#[derive(Clone, Copy)]
enum MathStyle {
    DoubleStruck,
    Script,
    Fraktur,
    Bold,
    SansSerif,
    Monospace,
    Italic,
    Roman,
}

/// The font-variant alphabet a styled-command name selects, or `None` for a wrapper that carries no
/// glyph change of its own (a math-class or presentation wrapper renders its argument transparently).
fn math_style(name: &str) -> Option<MathStyle> {
    Some(match name {
        "mathbb" | "mathds" => MathStyle::DoubleStruck,
        "mathcal" | "mathscr" => MathStyle::Script,
        "mathfrak" => MathStyle::Fraktur,
        "mathbf" => MathStyle::Bold,
        "mathsf" => MathStyle::SansSerif,
        "mathtt" => MathStyle::Monospace,
        "mathit" => MathStyle::Italic,
        "mathrm" | "mathup" => MathStyle::Roman,
        _ => return None,
    })
}

/// Restyle every identifier and number leaf beneath a node into a font-variant alphabet: each glyph
/// is mapped to its styled code point and tagged with the matching `mathvariant`. An operator leaf,
/// and a leaf that already carries a variant, is left as it is, so an inner style wins over an outer.
fn apply_math_style(node: &mut Node, style: MathStyle) {
    let Node::Element(element) = node else {
        return;
    };
    if matches!(element.name, "mi" | "mn") {
        restyle_token(element, style);
        return;
    }
    for child in &mut element.children {
        apply_math_style(child, style);
    }
}

/// Map a token's glyphs into a font-variant alphabet in place, tagging it with the `mathvariant`. A
/// leaf that already carries a variant, or that holds a character with no styled form in the target
/// alphabet, is left untouched.
fn restyle_token(element: &mut Element, style: MathStyle) {
    if element
        .attributes
        .iter()
        .any(|(key, _)| *key == "mathvariant")
    {
        return;
    }
    let Some(text) = token_text(element) else {
        return;
    };
    let mut glyph = String::new();
    let mut variant: Option<&'static str> = None;
    for ch in text.chars() {
        let Some((styled, next)) = style_char(style, ch) else {
            return;
        };
        if variant.is_some_and(|current| current != next) {
            return;
        }
        variant = Some(next);
        glyph.push_str(&styled);
    }
    if let Some(variant) = variant {
        element.children = vec![Node::Text(glyph)];
        element
            .attributes
            .push(("mathvariant", variant.to_string()));
    }
}

/// The single text run a leaf carries, or `None` if it is empty or holds anything but one text node.
fn token_text(element: &Element) -> Option<String> {
    if element.children.len() != 1 {
        return None;
    }
    match element.children.first() {
        Some(Node::Text(text)) if !text.is_empty() => Some(text.clone()),
        _ => None,
    }
}

/// A single character's styled glyph and `mathvariant` under a font style, or `None` where the style
/// leaves the character unchanged (a symbol with no form in the target alphabet).
fn style_char(style: MathStyle, ch: char) -> Option<(String, &'static str)> {
    match style {
        MathStyle::Roman => Some((ch.to_string(), "normal")),
        MathStyle::DoubleStruck => symbols::styled_letter(symbols::Alphabet::DoubleStruck, ch)
            .map(|g| (g, "double-struck")),
        MathStyle::Script => {
            symbols::styled_letter(symbols::Alphabet::Script, ch).map(|g| (g, "script"))
        }
        MathStyle::Fraktur => {
            symbols::styled_letter(symbols::Alphabet::Fraktur, ch).map(|g| (g, "fraktur"))
        }
        MathStyle::SansSerif => {
            block_glyph(ch, 0x1D5A0, 0x1D5BA, Some(0x1D7E2)).map(|g| (g, "sans-serif"))
        }
        MathStyle::Monospace => {
            block_glyph(ch, 0x1D670, 0x1D68A, Some(0x1D7F6)).map(|g| (g, "monospace"))
        }
        MathStyle::Italic => italic_glyph(ch).map(|g| (g, "italic")),
        MathStyle::Bold => bold_glyph(ch),
    }
}

/// A letter or digit glyph from a contiguous Mathematical Alphanumeric block, given the block's base
/// code points for `A`, `a`, and (where the block styles digits) `0`.
fn block_glyph(ch: char, upper: u32, lower: u32, digit: Option<u32>) -> Option<String> {
    let code = if ch.is_ascii_uppercase() {
        upper + (ch as u32 - 'A' as u32)
    } else if ch.is_ascii_lowercase() {
        lower + (ch as u32 - 'a' as u32)
    } else if ch.is_ascii_digit() {
        digit? + (ch as u32 - '0' as u32)
    } else {
        return None;
    };
    char::from_u32(code).map(|c| c.to_string())
}

/// The italic form of a Latin letter. Italic small `h` has no place in the block and takes the
/// Planck-constant glyph instead; a digit keeps its upright form.
fn italic_glyph(ch: char) -> Option<String> {
    if ch == 'h' {
        return Some('\u{210E}'.to_string());
    }
    block_glyph(ch, 0x1D434, 0x1D44E, None)
}

/// The bold form of a character: a Latin letter takes the bold-italic block that `\mathbf` renders, a
/// Greek letter the bold Greek block, and a digit the bold digit block.
fn bold_glyph(ch: char) -> Option<(String, &'static str)> {
    if ch.is_ascii_alphabetic() {
        return block_glyph(ch, 0x1D468, 0x1D482, None).map(|g| (g, "bold-italic"));
    }
    if ch.is_ascii_digit() {
        return block_glyph(ch, 0, 0, Some(0x1D7CE)).map(|g| (g, "bold"));
    }
    bold_greek(ch).map(|g| (g, "bold"))
}

/// The bold form of a Greek letter, from the bold Greek block that mirrors the Greek layout.
fn bold_greek(ch: char) -> Option<String> {
    let code = ch as u32;
    let mapped = if (0x391..=0x3A9).contains(&code) {
        0x1D6A8 + (code - 0x391)
    } else if (0x3B1..=0x3C9).contains(&code) {
        0x1D6C2 + (code - 0x3B1)
    } else {
        return None;
    };
    char::from_u32(mapped).map(|c| c.to_string())
}

fn text(name: &str, pieces: &[TextPiece]) -> Node {
    let mut content = String::new();
    for piece in pieces {
        match piece {
            TextPiece::Run(run) => content.push_str(run),
            TextPiece::Space(space) => content.push(space.codepoint()),
            TextPiece::Math(_) => {}
        }
    }
    let mut element = Element::new("mtext").text(content);
    if name != "textit" && name != "emph" {
        element = element.attr("mathvariant", "normal");
    }
    Node::Element(element)
}

fn binomial(top: &[Atom], bottom: &[Atom], display: bool, depth: usize) -> Node {
    let stack = Node::Element(
        Element::new("mfrac")
            .attr("linethickness", "0")
            .node(slot(top, display, depth))
            .node(slot(bottom, display, depth)),
    );
    fenced("(", ")", vec![stack])
}

fn matrix(delimiter: MatrixDelim, rows: &[Vec<Vec<Atom>>], display: bool, depth: usize) -> Node {
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

fn grid(
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
    if index % 2 == 0 { even } else { odd }
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

// ----------------------------------------------------------------------------
// Braces, stacks, arrows
// ----------------------------------------------------------------------------

/// Lower a horizontal brace: the group under (or over) its brace glyph, with a matching-side label
/// (a superscript over an over-brace, a subscript under an under-brace) stacked as an outer limit.
fn brace_atom(
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

    // The matching-side script (a superscript over an over-brace, a subscript under an under-brace)
    // becomes the brace's stacked label; any other script applies as an ordinary script.
    let (label, remaining_sub, remaining_sup) = match kind {
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
    if remaining_sub.is_some() || remaining_sup.is_some() || !atom.siblings.is_empty() {
        node = apply_scripts(node, remaining_sub, remaining_sup, false, display, depth);
        for sibling in &atom.siblings {
            node = apply_sibling(node, sibling, false, display, depth);
        }
    }
    vec![node]
}

fn stack_over_under(
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

fn ext_arrow(
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

// ----------------------------------------------------------------------------
// Delimiters and negation
// ----------------------------------------------------------------------------

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

fn delimited(
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

fn big_delimiter(scale: u16, inner: &Atom, display: bool, depth: usize) -> Vec<Node> {
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
fn delimiter_glyph(delimiter: Delim, opening: bool) -> String {
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
fn negated(base: &str) -> Node {
    if let Some(glyph) = symbols::negated_relation(base) {
        return leaf("mo", glyph.to_string());
    }
    match super::inlines::negated_base(base) {
        Some(super::inlines::NegatedBase::Relation(mut glyph)) => {
            glyph.push('\u{0338}');
            leaf("mo", glyph)
        }
        Some(super::inlines::NegatedBase::Italic(mut glyph)) => {
            glyph.push('\u{0338}');
            leaf("mi", glyph)
        }
        Some(super::inlines::NegatedBase::Upright(mut glyph)) => {
            glyph.push('\u{0338}');
            leaf("mn", glyph)
        }
        None => leaf("mi", base.to_string()),
    }
}

fn negated_group(atoms: &[Atom], display: bool, depth: usize) -> Node {
    Node::Element(
        Element::new("mover")
            .attr("accent", "true")
            .node(slot(atoms, display, depth))
            .node(Node::Element(
                Element::new("mo").attr("accent", "true").text("\u{0338}"),
            )),
    )
}

// ----------------------------------------------------------------------------
// Modulo
// ----------------------------------------------------------------------------

/// Lower a modulo operator to its node sequence: a leading space, an optional opening parenthesis,
/// the `mod` word with a function-application marker, a following space, the bracketed modulus for the
/// parenthesised forms, and a closing parenthesis.
fn modulo(kind: ModKind, argument: Option<&[Atom]>, display: bool, depth: usize) -> Vec<Node> {
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
