//! Styled-alphabet, math-class, and text-mode wrapper lowering for the OMML math backend.

use super::super::parse::{Atom, Body, TextPiece};
use super::{
    Element, Ink, ItalicAxis, Style, command_glyph, leaf, lower_seq, non_empty, operator_box,
    properties, run, style_value,
};

/// The style a styled-alphabet or math-class wrapper imposes on its argument. A class wrapper sets
/// its argument upright; an alphabet wrapper selects a script variant and bold/italic axes. An
/// unsupported wrapper reports the expression unconvertible.
pub(super) fn styled_style(name: &str, current: Style) -> Option<Style> {
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

/// Renders relation spacing for glyphs and text while preserving structured arguments.
pub(super) fn relation(argument: &[Atom], style: Style, depth: usize) -> Option<Vec<Element>> {
    let mut inner = argument;
    while let [atom] = inner {
        if atom.sub.is_some() || atom.sup.is_some() || !atom.siblings.is_empty() {
            break;
        }
        let glyph = match &atom.body {
            Body::Group(children) => {
                inner = children;
                continue;
            }
            Body::Styled(name, children)
                if matches!(
                    name.as_str(),
                    "mathord"
                        | "mathrel"
                        | "mathop"
                        | "mathbin"
                        | "mathopen"
                        | "mathclose"
                        | "mathpunct"
                ) =>
            {
                inner = children;
                continue;
            }
            Body::Char(character) => character.to_string(),
            Body::Command(name) => {
                let Some((glyph, _)) = command_glyph(name) else {
                    break;
                };
                glyph
            }
            Body::Text(name, pieces) => {
                if let Some(mut inner_style) = text_style(name) {
                    let mut word = String::new();
                    for piece in pieces {
                        match piece {
                            TextPiece::Run(literal) => word.push_str(literal),
                            TextPiece::Space(space) => word.push(space.codepoint()),
                            TextPiece::Math(_) => {
                                return lower_seq(argument, style, depth + 1, false).map(non_empty);
                            }
                        }
                    }
                    if !word.is_empty() {
                        inner_style.normal_text = false;
                        return Some(vec![operator_box(leaf(&word, Ink::Upright, inner_style))]);
                    }
                }
                break;
            }
            _ => break,
        };
        if glyph.chars().count() == 1 && glyph.chars().all(char::is_alphabetic) {
            let inner_style = styled_style("mathrel", style)?;
            return Some(vec![operator_box(leaf(&glyph, Ink::Upright, inner_style))]);
        }
        break;
    }
    lower_seq(argument, style, depth + 1, false).map(non_empty)
}

/// Lower a text-mode wrapper. `\operatorname` folds to a single upright run; the `\text` family sets
/// each literal run in normal text with the wrapper's formatting and switches back to math mode for
/// any embedded sub-expression.
pub(super) fn text(name: &str, pieces: &[TextPiece], depth: usize) -> Option<Vec<Element>> {
    if name == "operatorname" || name == "operatorname*" {
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
            TextPiece::Math(atoms) => {
                out.append(&mut lower_seq(atoms, Style::PLAIN, depth + 1, false)?);
            }
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
