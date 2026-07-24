//! Glyph, spacing, and n-ary operator lowering for the OMML math backend.

use super::super::parse::{Atom, Body};
use super::super::symbols;
use super::{
    Element, Ink, Style, ZERO_WIDTH_SPACE, filler, leaf, lower_atom, non_empty, properties, run,
    script_slot, style_value, wrap,
};

/// Lower a control-sequence nucleus: an inter-atom spacing, a Greek letter, a symbol, or a named
/// operator. An unknown command has no rendering and reports the expression unconvertible.
pub(super) fn command_nucleus(name: &str, style: Style) -> Option<Vec<Element>> {
    if let Some((text, upright)) = spacing(name) {
        let properties = upright.then(|| properties(vec![style_value("p")]));
        return Some(vec![run(text, properties)]);
    }
    let (text, ink) = command_glyph(name)?;
    Some(vec![leaf(&text, ink, style)])
}

/// A single source character's glyph text and ink.
pub(super) fn char_glyph(c: char) -> (String, Ink) {
    // In math a hyphen-minus is the subtraction/negation operator, drawn with the minus-sign glyph.
    if c == '-' {
        return ("\u{2212}".to_string(), Ink::Upright);
    }
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
pub(super) fn command_glyph(name: &str) -> Option<(String, Ink)> {
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
pub(super) fn prime_marks(count: u8) -> String {
    match count {
        1 => "\u{2032}".to_string(),
        2 => "\u{2033}".to_string(),
        3 => "\u{2034}".to_string(),
        4 => "\u{2057}".to_string(),
        other => "\u{2032}".repeat(usize::from(other)),
    }
}

/// The `:=` relation, boxed so the two glyphs set as one operator.
pub(super) fn colon_equals(style: Style) -> Element {
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

/// An n-ary operator's glyph and limit placement, when the body is one. The large operators that are
/// not n-ary (`\bigcup`, `\bigoplus`, …) render as ordinary scripted glyphs instead.
pub(super) fn n_ary_operator(body: &Body) -> Option<(char, &'static str)> {
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

pub(super) fn n_ary_element(
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
        Some(operand) => non_empty(lower_atom(operand, style, depth + 1, false)?),
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
