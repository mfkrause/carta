//! Office Math Markup (OMML) backend: lowers the shared math parse tree ([`super::parse`]) to the
//! `m:`-namespaced math grammar used by the OOXML word-processing format.
//!
//! The tree is the same one the other backends consume; here it is walked into an element tree and
//! serialized. Every construct the tree can carry is either rendered to its OMML shape or, when it
//! has no faithful single-line-of-source OMML form yet, reported as unconvertible by returning
//! `None` so the caller can emit the verbatim source instead. The walk is panic-free and bounded
//! against pathological nesting by an explicit depth limit.
//!
//! Word sets math italics automatically: a run with no style property renders latin letters and
//! lowercase Greek in italic and everything else upright. A glyph that must defy that default
//! carries an explicit style property — `<m:sty m:val="p"/>` forces upright, and a styled-alphabet
//! wrapper (`\mathbb`, `\mathbf`, …) spells out its bold/italic and script variant on every run.

use super::parse::{
    self, Atom, BinomKind, Body, Delim, GridKind, MatrixDelim, RunScript, ScriptKind, TextPiece,
};
use super::symbols;

/// Maximum structural nesting depth before the walk gives up and reports the input as
/// unconvertible. The parser already bounds brace nesting well below this, so a valid expression
/// never reaches it.
const MAX_DEPTH: usize = 256;

/// The zero-width space that fills an otherwise-empty script slot or nucleus, so every structural
/// element that requires content still has a run to hold.
const ZERO_WIDTH_SPACE: &str = "\u{200B}";

/// Convert TeX math source to an OMML fragment: an `<m:oMath>` element for inline math, or an
/// `<m:oMathPara>` wrapper with centered justification for display math. Returns `None` when the
/// source cannot be parsed or contains a construct with no OMML rendering, so the caller can emit
/// the verbatim source.
pub(crate) fn to_omml(tex: &str, display: bool) -> Option<String> {
    let atoms = parse::parse(tex)?;
    let body = lower_seq(&atoms, Style::PLAIN, 0)?;
    let math = wrap("m:oMath", body);
    let root = if display {
        Element::new("m:oMathPara")
            .child(
                Element::new("m:oMathParaPr").child(Element::new("m:jc").attr("m:val", "center")),
            )
            .child(math)
    } else {
        math
    };
    let mut out = String::new();
    root.render(&mut out);
    Some(out)
}

// ----------------------------------------------------------------------------
// Minimal XML element tree
// ----------------------------------------------------------------------------

/// An XML element node: a tag, its ordered attributes, and its ordered children. Empty elements
/// serialize self-closed (`<m:deg />`); text is escaped for element content, attribute values for
/// attribute context.
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

    fn child(mut self, child: Element) -> Self {
        self.children.push(Node::Element(child));
        self
    }

    fn text(mut self, text: impl Into<String>) -> Self {
        self.children.push(Node::Text(text.into()));
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
        if self.children.is_empty() {
            out.push_str(" />");
            return;
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

fn escape_text(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
}

fn escape_attribute(value: &str, out: &mut String) {
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '"' => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
}

/// Build an element with the given element children in order.
fn wrap(name: &'static str, children: Vec<Element>) -> Element {
    let mut element = Element::new(name);
    for child in children {
        element = element.child(child);
    }
    element
}

/// A run: an optional run-properties element followed by the text element.
fn run(text: &str, properties: Option<Element>) -> Element {
    let mut run = Element::new("m:r");
    if let Some(properties) = properties {
        run = run.child(properties);
    }
    run.child(Element::new("m:t").text(text))
}

/// The zero-width-space filler run.
fn filler() -> Element {
    run(ZERO_WIDTH_SPACE, None)
}

/// Fall back to a single filler run when a required-content slot lowered to nothing.
fn non_empty(mut runs: Vec<Element>) -> Vec<Element> {
    if runs.is_empty() {
        runs.push(filler());
    }
    runs
}

// ----------------------------------------------------------------------------
// Run styling
// ----------------------------------------------------------------------------

/// Whether a glyph is italic, an upright digit, or upright by default under Word's automatic math
/// italicization. The three collapse to two run-property outcomes at the top level (only `Upright`
/// forces a style property) but stay distinct inside a styled wrapper, where a digit and an operator
/// both stay non-italic while a letter italicizes.
#[derive(Clone, Copy)]
enum Ink {
    Italic,
    Digit,
    Upright,
}

/// How a styled wrapper sets the italic axis: follow the glyph's automatic default, or force it.
#[derive(Clone, Copy)]
enum ItalicAxis {
    Auto,
    Force(bool),
}

/// The active run style. Outside a wrapper (`explicit` false) a run carries a style property only to
/// force upright; inside one, every run spells out its script variant and bold/italic axes.
#[derive(Clone, Copy)]
struct Style {
    explicit: bool,
    /// A `<m:nor/>` normal-text marker, set by the text wrappers.
    normal_text: bool,
    /// The `<m:scr>` alphabet variant (`double-struck`, `script`, `fraktur`, `sans-serif`,
    /// `monospace`), when the wrapper selects one.
    script: Option<&'static str>,
    bold: bool,
    italic: ItalicAxis,
}

impl Style {
    const PLAIN: Style = Style {
        explicit: false,
        normal_text: false,
        script: None,
        bold: false,
        italic: ItalicAxis::Auto,
    };

    /// The base for a styled/text wrapper: an explicit run that is upright unless the wrapper says
    /// otherwise.
    const WRAPPER: Style = Style {
        explicit: true,
        normal_text: false,
        script: None,
        bold: false,
        italic: ItalicAxis::Force(false),
    };
}

/// Build the run-properties element for a leaf glyph, or `None` when the run needs none.
fn leaf_properties(ink: Ink, style: Style) -> Option<Element> {
    if !style.explicit {
        return match ink {
            Ink::Upright => Some(properties(vec![style_value("p")])),
            Ink::Italic | Ink::Digit => None,
        };
    }
    let mut parts = Vec::new();
    if style.normal_text {
        parts.push(Element::new("m:nor"));
    }
    if let Some(script) = style.script {
        parts.push(Element::new("m:scr").attr("m:val", script));
    }
    let italic = match style.italic {
        ItalicAxis::Force(value) => value,
        ItalicAxis::Auto => matches!(ink, Ink::Italic),
    };
    parts.push(style_value(match (style.bold, italic) {
        (true, true) => "bi",
        (true, false) => "b",
        (false, true) => "i",
        (false, false) => "p",
    }));
    Some(properties(parts))
}

fn properties(parts: Vec<Element>) -> Element {
    wrap("m:rPr", parts)
}

fn style_value(value: &'static str) -> Element {
    Element::new("m:sty").attr("m:val", value)
}

/// A styled leaf run: the glyph with run properties computed from its ink and the active style.
fn leaf(text: &str, ink: Ink, style: Style) -> Element {
    run(text, leaf_properties(ink, style))
}

// ----------------------------------------------------------------------------
// Sequence and atom lowering
// ----------------------------------------------------------------------------

/// Lower a run of atoms to a run of elements, folding an n-ary operator together with the operand
/// that follows it.
fn lower_seq(atoms: &[Atom], style: Style, depth: usize) -> Option<Vec<Element>> {
    if depth > MAX_DEPTH {
        return None;
    }
    let mut out = Vec::new();
    let mut index = 0;
    while let Some(atom) = atoms.get(index) {
        if let Some((glyph, limit_location)) = n_ary_operator(&atom.body)
            && (atom.sub.is_some() || atom.sup.is_some())
            && atom.siblings.is_empty()
        {
            let operand = atoms.get(index + 1);
            out.push(n_ary_element(
                glyph,
                limit_location,
                atom,
                operand,
                style,
                depth,
            )?);
            index += if operand.is_some() { 2 } else { 1 };
            continue;
        }
        out.append(&mut lower_atom(atom, style, depth)?);
        index += 1;
    }
    Some(out)
}

/// Lower one atom: its nucleus wrapped by whatever script chain it carries.
fn lower_atom(atom: &Atom, style: Style, depth: usize) -> Option<Vec<Element>> {
    let base = nucleus(&atom.body, style, depth)?;
    let runs = atom.script_runs();
    if runs.is_empty() {
        return Some(base);
    }
    // Each run either packs onto the accumulating base (a paired or sealed script) or restarts on a
    // fresh empty base (a flat-chain sibling), which is emitted as a following sibling element.
    let mut out = Vec::new();
    let mut current = base;
    for script_run in &runs {
        if script_run.restart {
            out.append(&mut current);
            current = vec![filler()];
        }
        current = vec![apply_scripts(current, &script_run.scripts, style, depth)?];
    }
    out.append(&mut current);
    Some(out)
}

/// Wrap a base in a subscript, superscript, or both, per the scripts of one render run.
fn apply_scripts(
    base: Vec<Element>,
    scripts: &[RunScript<'_>],
    style: Style,
    depth: usize,
) -> Option<Element> {
    let mut sub = None;
    let mut sup = None;
    for script in scripts {
        match script.kind {
            ScriptKind::Sub => sub = Some(script.atoms),
            ScriptKind::Sup => sup = Some(script.atoms),
        }
    }
    let nucleus = wrap("m:e", non_empty(base));
    Some(match (sub, sup) {
        (Some(sub), Some(sup)) => Element::new("m:sSubSup")
            .child(nucleus)
            .child(wrap("m:sub", script_slot(sub, style, depth)?))
            .child(wrap("m:sup", script_slot(sup, style, depth)?)),
        (Some(sub), None) => Element::new("m:sSub")
            .child(nucleus)
            .child(wrap("m:sub", script_slot(sub, style, depth)?)),
        (None, Some(sup)) => Element::new("m:sSup")
            .child(nucleus)
            .child(wrap("m:sup", script_slot(sup, style, depth)?)),
        (None, None) => return None,
    })
}

/// Lower the atoms of a script slot, filling an empty slot with a zero-width space.
fn script_slot(atoms: &[Atom], style: Style, depth: usize) -> Option<Vec<Element>> {
    Some(non_empty(lower_seq(atoms, style, depth + 1)?))
}

/// Lower an atom's nucleus (its body without scripts) to zero or more runs.
fn nucleus(body: &Body, style: Style, depth: usize) -> Option<Vec<Element>> {
    match body {
        Body::Char(c) => {
            let (text, ink) = char_glyph(*c);
            Some(vec![leaf(&text, ink, style)])
        }
        Body::Number(digits) => Some(vec![leaf(digits, Ink::Digit, style)]),
        Body::Empty | Body::EmptyGroup => Some(vec![filler()]),
        Body::Prime(count) => Some(vec![leaf(&prime_marks(*count), Ink::Upright, style)]),
        Body::ColonEq => Some(vec![colon_equals(style)]),
        Body::Command(name) => command_nucleus(name, style),
        Body::Group(atoms) => lower_seq(atoms, style, depth + 1),
        Body::Frac(numerator, denominator) => {
            Some(vec![fraction(numerator, denominator, "bar", style, depth)?])
        }
        Body::Sqrt(index, radicand) => {
            Some(vec![radical(index.as_deref(), radicand, style, depth)?])
        }
        Body::Accent(name, base) => Some(vec![accent(name, base, style, depth)?]),
        Body::Styled(name, argument) => match styled_style(name, style) {
            Some(inner) => lower_seq(argument, inner, depth + 1),
            None => None,
        },
        Body::Text(name, pieces) => text(name, pieces, depth),
        Body::Binom(kind, top, bottom) => Some(vec![binomial(*kind, top, bottom, style, depth)?]),
        Body::Matrix(delimiter, rows) => Some(vec![matrix(*delimiter, rows, style, depth)?]),
        Body::Delimited(open, close, content) => {
            Some(vec![delimited(*open, *close, content, style, depth)?])
        }
        Body::Grid(kind, rows) => grid(*kind, rows, style, depth),
        Body::Big(_, inner) => nucleus(&inner.body, style, depth),
        Body::Label(_) => Some(Vec::new()),
        Body::Middle(_, _)
        | Body::Mod(_, _)
        | Body::Negated(_)
        | Body::NegatedGroup(_)
        | Body::Brace(_, _)
        | Body::Stack(_, _, _)
        | Body::ExtArrow(_, _, _) => None,
    }
}

/// Lower a control-sequence nucleus: an inter-atom spacing, a Greek letter, a symbol, or a named
/// operator. An unknown command has no rendering and reports the expression unconvertible.
fn command_nucleus(name: &str, style: Style) -> Option<Vec<Element>> {
    if let Some((text, upright)) = spacing(name) {
        let properties = upright.then(|| properties(vec![style_value("p")]));
        return Some(vec![run(text, properties)]);
    }
    let (text, ink) = command_glyph(name)?;
    Some(vec![leaf(&text, ink, style)])
}

// ----------------------------------------------------------------------------
// Leaf glyphs
// ----------------------------------------------------------------------------

/// A single source character's glyph text and ink.
fn char_glyph(c: char) -> (String, Ink) {
    let ink = if c.is_ascii_digit() {
        Ink::Digit
    } else if c.is_ascii_alphabetic() || is_lowercase_greek(c) {
        Ink::Italic
    } else {
        Ink::Upright
    };
    (c.to_string(), ink)
}

/// A control sequence's glyph text and ink, from the Greek, symbol, and named-operator tables.
fn command_glyph(name: &str) -> Option<(String, Ink)> {
    if let Some((glyph, _)) = symbols::greek(name) {
        return Some((glyph.to_string(), greek_ink(glyph)));
    }
    if let Some(symbol) = symbols::symbol(name) {
        let ink = if symbol.italic {
            Ink::Italic
        } else {
            Ink::Upright
        };
        return Some((symbol.text.to_string(), ink));
    }
    if let Some((word, _)) = symbols::named_function(name) {
        return Some((word.to_string(), Ink::Upright));
    }
    None
}

/// A Greek glyph italicizes by default unless it is an uppercase letter.
fn greek_ink(glyph: &str) -> Ink {
    match glyph.chars().next() {
        Some(c) if ('\u{0391}'..='\u{03A9}').contains(&c) => Ink::Upright,
        _ => Ink::Italic,
    }
}

fn is_lowercase_greek(c: char) -> bool {
    ('\u{03B1}'..='\u{03C9}').contains(&c)
        || matches!(
            c,
            '\u{03D1}' | '\u{03D5}' | '\u{03D6}' | '\u{03F0}' | '\u{03F1}' | '\u{03F5}'
        )
}

/// The precomposed prime run for `count` marks, extending past the four precomposed glyphs by
/// repeating the single prime.
fn prime_marks(count: u8) -> String {
    match count {
        1 => "\u{2032}".to_string(),
        2 => "\u{2033}".to_string(),
        3 => "\u{2034}".to_string(),
        4 => "\u{2057}".to_string(),
        other => "\u{2032}".repeat(usize::from(other)),
    }
}

/// The `:=` relation, boxed so the two glyphs set as one operator.
fn colon_equals(style: Style) -> Element {
    Element::new("m:box")
        .child(Element::new("m:boxPr").child(Element::new("m:opEmu").attr("m:val", "on")))
        .child(wrap("m:e", vec![leaf(":=", Ink::Upright, style)]))
}

/// An inter-atom spacing's glyph and whether it carries an upright style property. Unknown or
/// zero-glyph spacings are absent, so the expression falls back to verbatim.
fn spacing(name: &str) -> Option<(&'static str, bool)> {
    Some(match name {
        "," => ("\u{2009}", false),
        ";" | "enspace" => ("\u{2004}", false),
        ":" | ">" | " " => ("\u{2005}", false),
        "!" => (ZERO_WIDTH_SPACE, false),
        "quad" => ("\u{2001}", false),
        "qquad" => ("\u{2001}\u{2001}", false),
        "medspace" => ("\u{205F}", true),
        _ => return None,
    })
}

// ----------------------------------------------------------------------------
// N-ary operators
// ----------------------------------------------------------------------------

/// An n-ary operator's glyph and limit placement, when the body is one. The large operators that are
/// not n-ary (`\bigcup`, `\bigoplus`, …) render as ordinary scripted glyphs instead.
fn n_ary_operator(body: &Body) -> Option<(char, &'static str)> {
    match body {
        Body::Command(name) => n_ary_named(name),
        Body::Char(c) => n_ary_glyph(*c),
        _ => None,
    }
}

fn n_ary_named(name: &str) -> Option<(char, &'static str)> {
    Some(match name {
        "sum" => ('\u{2211}', "undOvr"),
        "prod" => ('\u{220F}', "undOvr"),
        "coprod" => ('\u{2210}', "undOvr"),
        "int" => ('\u{222B}', "subSup"),
        "iint" => ('\u{222C}', "subSup"),
        "iiint" => ('\u{222D}', "subSup"),
        "oint" => ('\u{222E}', "subSup"),
        "oiint" => ('\u{222F}', "subSup"),
        "oiiint" => ('\u{2230}', "subSup"),
        _ => return None,
    })
}

fn n_ary_glyph(c: char) -> Option<(char, &'static str)> {
    Some(match c {
        '\u{2211}' | '\u{220F}' | '\u{2210}' => (c, "undOvr"),
        '\u{222B}' | '\u{222C}' | '\u{222D}' | '\u{222E}' | '\u{222F}' | '\u{2230}' => {
            (c, "subSup")
        }
        _ => return None,
    })
}

fn n_ary_element(
    glyph: char,
    limit_location: &'static str,
    atom: &Atom,
    operand: Option<&Atom>,
    style: Style,
    depth: usize,
) -> Option<Element> {
    let properties = Element::new("m:naryPr")
        .child(Element::new("m:chr").attr("m:val", glyph.to_string()))
        .child(Element::new("m:limLoc").attr("m:val", limit_location))
        .child(hide("m:subHide", atom.sub.is_none()))
        .child(hide("m:supHide", atom.sup.is_none()));
    let sub = optional_slot(atom.sub.as_deref(), style, depth)?;
    let sup = optional_slot(atom.sup.as_deref(), style, depth)?;
    let operand = match operand {
        Some(operand) => non_empty(lower_atom(operand, style, depth + 1)?),
        None => vec![filler()],
    };
    Some(
        Element::new("m:nary")
            .child(properties)
            .child(wrap("m:sub", sub))
            .child(wrap("m:sup", sup))
            .child(wrap("m:e", operand)),
    )
}

fn hide(name: &'static str, hidden: bool) -> Element {
    Element::new(name).attr("m:val", if hidden { "on" } else { "off" })
}

/// Lower an optional script slot to its runs, or a lone filler when absent.
fn optional_slot(atoms: Option<&[Atom]>, style: Style, depth: usize) -> Option<Vec<Element>> {
    match atoms {
        Some(atoms) => script_slot(atoms, style, depth),
        None => Some(vec![filler()]),
    }
}

// ----------------------------------------------------------------------------
// Two-dimensional structures
// ----------------------------------------------------------------------------

fn fraction(
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

fn radical(
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

fn accent(name: &str, base: &[Atom], style: Style, depth: usize) -> Option<Element> {
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

fn binomial(
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

/// A stretchable delimiter fence around some content.
fn fence(open: &str, close: &str, content: Vec<Element>) -> Element {
    Element::new("m:d")
        .child(
            Element::new("m:dPr")
                .child(Element::new("m:begChr").attr("m:val", open))
                .child(Element::new("m:sepChr").attr("m:val", ""))
                .child(Element::new("m:endChr").attr("m:val", close))
                .child(Element::new("m:grow")),
        )
        .child(wrap("m:e", non_empty(content)))
}

fn delimited(
    open: Option<Delim>,
    close: Option<Delim>,
    content: &[Atom],
    style: Style,
    depth: usize,
) -> Option<Element> {
    // A `\middle` divider would split the fence into several slots, which this single-slot fence
    // cannot represent, so such a group falls back to verbatim.
    if content
        .iter()
        .any(|atom| matches!(atom.body, Body::Middle(_, _)))
    {
        return None;
    }
    let open = open.map_or("", |delimiter| delimiter_glyph(delimiter, true));
    let close = close.map_or("", |delimiter| delimiter_glyph(delimiter, false));
    Some(fence(open, close, lower_seq(content, style, depth + 1)?))
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

fn matrix(
    delimiter: MatrixDelim,
    rows: &[Vec<Vec<Atom>>],
    style: Style,
    depth: usize,
) -> Option<Element> {
    let grid = grid_body(rows, "center", style, depth)?;
    Some(match delimiter {
        MatrixDelim::None => grid,
        MatrixDelim::Paren => fence("(", ")", vec![grid]),
        MatrixDelim::Bracket => fence("[", "]", vec![grid]),
        MatrixDelim::Brace => fence("{", "}", vec![grid]),
        MatrixDelim::Bar => fence("\u{2223}", "\u{2223}", vec![grid]),
        MatrixDelim::DoubleBar => fence("\u{2225}", "\u{2225}", vec![grid]),
    })
}

/// Lower a grid environment: a case block fenced with a left brace, a stacked substack, or an
/// aligned block set as an equation array.
fn grid(
    kind: GridKind,
    rows: &[Vec<Vec<Atom>>],
    style: Style,
    depth: usize,
) -> Option<Vec<Element>> {
    Some(match kind {
        GridKind::Cases => vec![fence("{", "", vec![grid_body(rows, "left", style, depth)?])],
        GridKind::Substack => vec![grid_body(rows, "center", style, depth)?],
        GridKind::Aligned => vec![equation_array(rows, style, depth)?],
    })
}

/// The `<m:m>` matrix body: column properties for the widest row, then a matrix row per source row.
fn grid_body(
    rows: &[Vec<Vec<Atom>>],
    justification: &'static str,
    style: Style,
    depth: usize,
) -> Option<Element> {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut column_properties = Element::new("m:mcs");
    for _ in 0..columns {
        column_properties = column_properties.child(
            Element::new("m:mc").child(
                Element::new("m:mcPr")
                    .child(Element::new("m:mcJc").attr("m:val", justification))
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
            line.append(&mut lower_seq(cell, style, depth + 1)?);
        }
        array = array.child(wrap("m:e", non_empty(line)));
    }
    Some(array)
}

// ----------------------------------------------------------------------------
// Styled alphabets and text
// ----------------------------------------------------------------------------

/// The style a styled-alphabet or math-class wrapper imposes on its argument. A class wrapper sets
/// its argument upright; an alphabet wrapper selects a script variant and bold/italic axes. An
/// unsupported presentation wrapper (`\phantom`, `\boxed`, …) reports the expression unconvertible.
fn styled_style(name: &str, current: Style) -> Option<Style> {
    let base = Style::WRAPPER;
    Some(match name {
        "mathord" | "mathrel" | "mathop" | "mathbin" | "mathopen" | "mathclose" | "mathpunct" => {
            Style {
                explicit: true,
                italic: ItalicAxis::Force(false),
                ..current
            }
        }
        "mathbf" | "boldsymbol" | "bm" | "symbf" | "pmb" | "mathbfup" => Style {
            bold: true,
            italic: ItalicAxis::Auto,
            ..base
        },
        "mathbfit" => Style {
            bold: true,
            italic: ItalicAxis::Force(true),
            ..base
        },
        "mathit" => Style {
            italic: ItalicAxis::Force(true),
            ..base
        },
        "mathrm" | "mathup" => base,
        "mathbb" | "mathds" => Style {
            script: Some("double-struck"),
            ..base
        },
        "mathcal" | "mathscr" => Style {
            script: Some("script"),
            ..base
        },
        "mathfrak" => Style {
            script: Some("fraktur"),
            ..base
        },
        "mathsf" | "mathsfup" => Style {
            script: Some("sans-serif"),
            ..base
        },
        "mathtt" => Style {
            script: Some("monospace"),
            ..base
        },
        "mathsfit" => Style {
            script: Some("sans-serif"),
            italic: ItalicAxis::Force(true),
            ..base
        },
        "mathbfsfit" => Style {
            bold: true,
            script: Some("sans-serif"),
            italic: ItalicAxis::Force(true),
            ..base
        },
        "mathbfsfup" => Style {
            bold: true,
            script: Some("sans-serif"),
            ..base
        },
        "mathbfcal" | "mathbfscr" => Style {
            bold: true,
            script: Some("script"),
            ..base
        },
        "mathbffrak" => Style {
            bold: true,
            script: Some("fraktur"),
            ..base
        },
        _ => return None,
    })
}

/// Lower a text-mode wrapper. `\operatorname` folds to a single upright run; the `\text` family sets
/// each literal run in normal text with the wrapper's formatting and switches back to math mode for
/// any embedded sub-expression.
fn text(name: &str, pieces: &[TextPiece], depth: usize) -> Option<Vec<Element>> {
    if name == "operatorname" {
        let mut word = String::new();
        for piece in pieces {
            match piece {
                TextPiece::Run(literal) => word.push_str(literal),
                TextPiece::Space(space) => word.push(space.codepoint()),
                TextPiece::Math(_) => return None,
            }
        }
        return Some(vec![run(&word, Some(properties(vec![style_value("p")])))]);
    }
    let style = text_style(name)?;
    let mut out = Vec::new();
    for piece in pieces {
        match piece {
            TextPiece::Run(literal) => out.push(leaf(literal, Ink::Upright, style)),
            TextPiece::Space(space) => {
                out.push(leaf(&space.codepoint().to_string(), Ink::Upright, style));
            }
            TextPiece::Math(atoms) => out.append(&mut lower_seq(atoms, Style::PLAIN, depth + 1)?),
        }
    }
    Some(out)
}

/// The style a text wrapper sets: normal text with the wrapper's weight, slant, and family.
fn text_style(name: &str) -> Option<Style> {
    let base = Style {
        normal_text: true,
        ..Style::WRAPPER
    };
    Some(match name {
        "text" | "textrm" | "mbox" => base,
        "textbf" => Style { bold: true, ..base },
        "textit" => Style {
            italic: ItalicAxis::Force(true),
            ..base
        },
        "texttt" => Style {
            script: Some("monospace"),
            ..base
        },
        "textsf" => Style {
            script: Some("sans-serif"),
            ..base
        },
        _ => return None,
    })
}

#[cfg(test)]
mod tests;
