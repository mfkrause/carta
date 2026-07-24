//! Lowering of styled, text, and accent math wrappers to Typst markup.

use super::super::parse::{Atom, Body, TextPiece};
use super::lower;

#[allow(clippy::match_same_arms)]
pub(super) fn styled_str(display: bool, name: &str, arg: &[Atom]) -> Option<String> {
    let inner = lower(display, arg)?;
    let s = match name {
        "mathbb" | "mathds" => format!("bb({inner})"),
        "mathcal" | "mathscr" => format!("cal({inner})"),
        "mathfrak" => format!("frak({inner})"),
        "mathbf" => format!("upright(bold({inner}))"),
        "boldsymbol" | "bm" | "symbf" | "mathbfup" => format!("bold({inner})"),
        "mathit" => format!("italic({inner})"),
        "mathsf" | "mathsfup" => format!("sans({inner})"),
        "mathbfsfup" => format!("bold(sans({inner}))"),
        "mathtt" => format!("mono({inner})"),
        "mathbfit" => format!("bold(italic({inner}))"),
        "mathsfit" => format!("italic(sans({inner}))"),
        "mathbfsfit" => format!("bold(italic(sans({inner})))"),
        "mathbfcal" | "mathbfscr" => format!("bold(cal({inner}))"),
        "mathbffrak" => format!("bold(frak({inner}))"),
        "mathrm" | "mathup" => format!("upright({inner})"),
        "pmb" => format!("bold({inner})"),
        // Math-class wrappers re-class their argument but add no glyph: the content renders directly.
        "mathord" | "mathrel" | "mathbin" | "mathopen" | "mathclose" | "mathpunct" => inner,
        // `\mathop`: a multi-letter run becomes a known operator identifier or a quoted name;
        // anything else renders directly
        "mathop" => match operator_name(arg) {
            Some(name) if super::super::symbols::named_function(&name).is_some() => name,
            Some(name) => format!("\"{name}\""),
            None => inner,
        },
        "phantom" => format!("#hide[{inner}]"),
        "cancel" => format!("cancel({inner})"),
        "xcancel" => format!("cancel({inner}, cross: #true)"),
        "bcancel" => format!("cancel({inner}, inverted: #true)"),
        "boxed" => format!("#box(stroke: black, inset: 3pt, [$ {inner} $])"),
        "overparen" => format!("{inner}^paren.t"),
        "underparen" => format!("{inner}_paren.b"),
        _ => return None,
    };
    Some(s)
}

/// A run of plain letters joined into a single Typst identifier (e.g. `lim`), used for an operator
/// name. Returns `None` if any atom is not a bare letter, leaving the caller to fall back.
fn operator_name(atoms: &[Atom]) -> Option<String> {
    if atoms.len() < 2 {
        return None;
    }
    let mut out = String::new();
    for atom in atoms {
        if atom.sub.is_some() || atom.sup.is_some() || !atom.siblings.is_empty() {
            return None;
        }
        match &atom.body {
            Body::Char(c) if c.is_ascii_alphabetic() => out.push(*c),
            _ => return None,
        }
    }
    Some(out)
}

pub(super) fn text_str(display: bool, name: &str, content: &[TextPiece]) -> Option<String> {
    // `\operatorname`(*) folds spacing into one identifier or quoted string; other wrappers format
    // each run, spacing emitted as tokens between
    if name == "operatorname" || name == "operatorname*" {
        let text = text_run_text(content);
        let s = if super::super::symbols::named_function(&text).is_some() {
            text
        } else {
            format!("\"{}\"", escape_typst_string(&text))
        };
        return Some(s);
    }
    let wrapper = match name {
        "text" | "textrm" | "mbox" => "upright",
        "textbf" => "bold",
        "textit" => "italic",
        "texttt" => "mono",
        "textsf" => "sans",
        _ => return None,
    };
    let mut parts: Vec<String> = Vec::new();
    for piece in content {
        match piece {
            TextPiece::Run(run) if run.is_empty() => {
                // an empty segment still occupies a join position: contribute an empty token
                parts.push(String::new());
            }
            TextPiece::Run(run) => {
                parts.push(format!("{wrapper}(\"{}\")", escape_typst_string(run)));
            }
            TextPiece::Space(space) => parts.push(space.typst_token().to_string()),
            // A `$…$` segment is math, rendered unaffected by the wrapper's own formatting.
            TextPiece::Math(atoms) => parts.push(lower(display, atoms)?),
        }
    }
    // An empty wrapper still renders as the empty quoted form, matching a bare `\text{}`.
    if parts.is_empty() {
        return Some(format!("{wrapper}(\"\")"));
    }
    Some(parts.join(" "))
}

/// The concatenated literal text of a run sequence, with each spacing rendered as its codepoint. Used
/// for `\operatorname`, whose spacing folds into the single identifier rather than splitting it.
fn text_run_text(content: &[TextPiece]) -> String {
    let mut out = String::new();
    for piece in content {
        match piece {
            TextPiece::Run(run) => out.push_str(run),
            TextPiece::Space(space) => out.push(space.codepoint()),
            // A `$…$` cannot occur in an operator-name group, which is already math mode.
            TextPiece::Math(_) => {}
        }
    }
    out
}

/// Escape a literal string for inclusion in a Typst quoted string: backslash and double-quote.
fn escape_typst_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

pub(super) fn accent_func(name: &str) -> Option<&'static str> {
    let f = match name {
        "bar" => "macron",
        "hat" | "widehat" => "hat",
        "tilde" | "widetilde" => "tilde",
        "vec" | "overrightarrow" => "arrow",
        "overleftarrow" => "arrow.l",
        "dot" => "dot",
        "ddot" => "dot.double",
        "check" => "caron",
        "breve" => "breve",
        "acute" => "acute",
        "grave" => "grave",
        "mathring" => "circle",
        "overline" => "overline",
        "underline" => "underline",
        "overleftrightarrow" => "arrow.l.r",
        _ => return None,
    };
    Some(f)
}

/// The combining mark used by Typst's generic `accent(content, mark)` form for a multi-atom base.
pub(super) fn accent_mark(name: &str) -> Option<char> {
    let m = match name {
        "bar" => '\u{203E}',
        "hat" | "widehat" => '\u{0302}',
        "tilde" | "widetilde" => '\u{0303}',
        "vec" | "overrightarrow" => '\u{20D7}',
        "overleftarrow" => '\u{20D6}',
        "dot" => '\u{0307}',
        "ddot" => '\u{0308}',
        "check" => '\u{030C}',
        "breve" => '\u{0306}',
        "acute" => '\u{0301}',
        "grave" => '\u{0300}',
        "mathring" => '\u{030A}',
        "dddot" => '\u{20DB}',
        "ddddot" => '\u{20DC}',
        "overleftrightarrow" => '\u{20E1}',
        "underleftarrow" => '\u{20EE}',
        "underrightarrow" => '\u{20EF}',
        _ => return None,
    };
    Some(m)
}
