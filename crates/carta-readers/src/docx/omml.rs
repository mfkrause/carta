//! Office MathML to TeX rendering for the docx reader.

use crate::xml::{Element, local_name};

/// Renders an Office `MathML` element to a TeX string. The core constructs (fractions, scripts,
/// radicals, n-ary operators, delimiters, functions, accents, bars, matrices, and limits) are
/// mapped directly; anything unmodeled falls back to its rendered child content so no math is lost.
pub(super) fn omml_to_tex(element: &Element) -> String {
    let mut out = String::new();
    render_math_children(element, &mut out);
    out
}

fn render_math_children(element: &Element, out: &mut String) {
    for child in element.elements() {
        render_math(child, out);
    }
}

// The transparent-wrapper arm mirrors the wildcard to document known pass-throughs.
#[allow(clippy::too_many_lines, clippy::match_same_arms)]
fn render_math(element: &Element, out: &mut String) {
    match local_name(&element.name) {
        "r" | "t" => out.push_str(&map_math_text(&element.text())),
        "f" => {
            out.push_str("\\frac");
            push_group(element.child("num"), out);
            push_group(element.child("den"), out);
        }
        "sSup" => {
            push_base(element.child("e"), out);
            out.push('^');
            push_group(element.child("sup"), out);
        }
        "sSub" => {
            push_base(element.child("e"), out);
            out.push('_');
            push_group(element.child("sub"), out);
        }
        "sSubSup" => {
            push_base(element.child("e"), out);
            out.push('_');
            push_group(element.child("sub"), out);
            out.push('^');
            push_group(element.child("sup"), out);
        }
        "sPre" => {
            out.push_str("{}");
            out.push('_');
            push_group(element.child("sub"), out);
            out.push('^');
            push_group(element.child("sup"), out);
            push_base(element.child("e"), out);
        }
        "rad" => {
            let degree = element.child("deg").map(render_element).unwrap_or_default();
            if degree.is_empty() {
                out.push_str("\\sqrt");
            } else {
                out.push_str("\\sqrt[");
                out.push_str(&degree);
                out.push(']');
            }
            push_group(element.child("e"), out);
        }
        "nary" => render_nary(element, out),
        "d" => render_delimiter(element, out),
        "func" => {
            let name = element
                .child("fName")
                .map(render_element)
                .unwrap_or_default();
            out.push_str(&map_function(&name));
            out.push(' ');
            out.push_str(&element.child("e").map(render_element).unwrap_or_default());
        }
        "acc" => {
            let chr = element
                .child("accPr")
                .and_then(|pr| pr.child("chr"))
                .and_then(|element| element.attr("val"))
                .and_then(|value| value.chars().next())
                .unwrap_or('\u{0302}');
            out.push_str(accent_command(chr));
            push_group(element.child("e"), out);
        }
        "bar" => {
            let top = element
                .child("barPr")
                .and_then(|pr| pr.child("pos"))
                .and_then(|element| element.attr("val"))
                == Some("top");
            out.push_str(if top { "\\overline" } else { "\\underline" });
            push_group(element.child("e"), out);
        }
        "groupChr" => render_group_char(element, out),
        "m" => render_matrix(element, out),
        "limLow" => {
            push_base(element.child("e"), out);
            out.push('_');
            push_group(element.child("lim"), out);
        }
        "limUpp" => {
            push_base(element.child("e"), out);
            out.push('^');
            push_group(element.child("lim"), out);
        }
        "eqArr" => render_equation_array(element, out),
        // Boxes, phantoms, and other transparent wrappers contribute their content.
        "e" | "box" | "borderBox" | "num" | "den" | "sup" | "sub" | "deg" | "lim" | "fName"
        | "oMath" => render_math_children(element, out),
        _ => render_math_children(element, out),
    }
}

fn render_nary(element: &Element, out: &mut String) {
    let properties = element.child("naryPr");
    let chr = properties
        .and_then(|pr| pr.child("chr"))
        .and_then(|c| c.attr("val"))
        .and_then(|value| value.chars().next());
    out.push_str(nary_command(chr));
    let sub = element.child("sub").map(render_element).unwrap_or_default();
    if !sub.is_empty() {
        out.push('_');
        out.push('{');
        out.push_str(&sub);
        out.push('}');
    }
    let sup = element.child("sup").map(render_element).unwrap_or_default();
    if !sup.is_empty() {
        out.push('^');
        out.push('{');
        out.push_str(&sup);
        out.push('}');
    }
    out.push_str(&element.child("e").map(render_element).unwrap_or_default());
}

fn render_delimiter(element: &Element, out: &mut String) {
    let properties = element.child("dPr");
    let sep = properties
        .and_then(|pr| pr.child("sepChr"))
        .and_then(|c| c.attr("val"));
    // A missing fence defaults to a parenthesis; an explicitly empty one is a null delimiter.
    let beg = properties
        .and_then(|pr| pr.child("begChr"))
        .and_then(|c| c.attr("val"))
        .unwrap_or("(");
    let end = properties
        .and_then(|pr| pr.child("endChr"))
        .and_then(|c| c.attr("val"))
        .unwrap_or(")");
    let bodies: Vec<&Element> = element
        .elements()
        .filter(|child| local_name(&child.name) == "e")
        .collect();
    let rendered: Vec<String> = bodies.iter().map(|body| render_element(body)).collect();

    // Parens, brackets, and single bars stay unsized around short flat content; anything taller,
    // multi-compartment, or a scaling fence (braces, floors, angles) gets `\left … \right`.
    let sized = rendered.len() > 1
        || !plain_delimiter(beg)
        || !plain_delimiter(end)
        || bodies.iter().any(|body| tall_math(body));

    if !sized {
        let open = delimiter_token(beg);
        out.push_str(&open);
        if control_word(&open) {
            out.push(' ');
        }
        if let Some(inner) = rendered.first() {
            out.push_str(inner);
        }
        out.push_str(&delimiter_token(end));
        return;
    }

    out.push_str("\\left");
    out.push_str(&delimiter_token(beg));
    out.push(' ');
    for (index, inner) in rendered.iter().enumerate() {
        if index > 0 {
            // `\middle` only accepts a delimiter: a bar scales, anything else is written literally.
            match sep {
                None | Some("|") => out.push_str(" \\middle| "),
                Some(other) => out.push_str(other),
            }
        }
        out.push_str(inner);
    }
    out.push_str(" \\right");
    out.push_str(&delimiter_token(end));
}

/// Whether a fence character renders unsized when it surrounds short, flat content. The scalable
/// fences (braces, floors, ceilings, angles, double bars, the null delimiter) are excluded so they
/// always take `\left … \right`.
fn plain_delimiter(chr: &str) -> bool {
    matches!(chr, "(" | ")" | "[" | "]" | "|")
}

/// Whether a delimiter token is a control word, i.e. needs a following space so it does not run into
/// the next token (`\lbrack x`, not `\lbrackx`).
fn control_word(token: &str) -> bool {
    token.starts_with('\\')
        && token
            .chars()
            .last()
            .is_some_and(|c| c.is_ascii_alphabetic())
}

/// Whether an `m:e` compartment holds anything taller than a run of text, which forces an enclosing
/// delimiter to scale. Fractions, scripts, radicals, nested delimiters, accents and the like all
/// appear as a child element other than a run.
fn tall_math(body: &Element) -> bool {
    body.elements().any(|child| local_name(&child.name) != "r")
}

/// Renders an equation array (`m:eqArr`): each `m:e` is one right-aligned row of a TeX `array`.
fn render_equation_array(element: &Element, out: &mut String) {
    out.push_str("\\begin{array}{r}\n");
    let rows: Vec<String> = element
        .elements()
        .filter(|child| local_name(&child.name) == "e")
        .map(render_element)
        .collect();
    out.push_str(&rows.join(" \\\\\n"));
    out.push_str("\n\\end{array}");
}

fn render_group_char(element: &Element, out: &mut String) {
    let chr = element
        .child("groupChrPr")
        .and_then(|pr| pr.child("chr"))
        .and_then(|c| c.attr("val"))
        .and_then(|value| value.chars().next());
    let command = match chr {
        Some('\u{23DF}') => "\\underbrace",
        _ => "\\overbrace",
    };
    out.push_str(command);
    push_group(element.child("e"), out);
}

fn render_matrix(element: &Element, out: &mut String) {
    out.push_str("\\begin{matrix}\n");
    let rows: Vec<String> = element
        .elements()
        .filter(|child| local_name(&child.name) == "mr")
        .map(|row| {
            row.elements()
                .filter(|cell| local_name(&cell.name) == "e")
                .map(render_element)
                .collect::<Vec<_>>()
                .join(" & ")
        })
        .collect();
    out.push_str(&rows.join(" \\\\\n"));
    out.push_str("\n\\end{matrix}");
}

/// Renders one element's math content to a fresh string.
fn render_element(element: &Element) -> String {
    let mut out = String::new();
    render_math(element, &mut out);
    out
}

/// Renders an optional element and wraps it in `{}` as a script or fraction group.
fn push_group(element: Option<&Element>, out: &mut String) {
    out.push('{');
    if let Some(element) = element {
        render_math_children(element, out);
    }
    out.push('}');
}

/// Renders an optional base element without added braces.
fn push_base(element: Option<&Element>, out: &mut String) {
    if let Some(element) = element {
        render_math_children(element, out);
    }
}

/// Maps a delimiter character to its TeX token.
fn delimiter_token(chr: &str) -> String {
    match chr {
        "" => ".".to_owned(),
        "(" | ")" | "|" | "/" => chr.to_owned(),
        "[" => "\\lbrack".to_owned(),
        "]" => "\\rbrack".to_owned(),
        "{" => "\\{".to_owned(),
        "}" => "\\}".to_owned(),
        "‖" => "\\|".to_owned(),
        "⟨" => "\\langle".to_owned(),
        "⟩" => "\\rangle".to_owned(),
        "⌊" => "\\lfloor".to_owned(),
        "⌋" => "\\rfloor".to_owned(),
        "⌈" => "\\lceil".to_owned(),
        "⌉" => "\\rceil".to_owned(),
        other => other.to_owned(),
    }
}

/// Maps an n-ary operator character to its TeX command, defaulting to an integral.
#[allow(clippy::match_same_arms)]
fn nary_command(chr: Option<char>) -> &'static str {
    match chr {
        Some('∑') => "\\sum",
        Some('∏') => "\\prod",
        Some('∐') => "\\coprod",
        Some('∫') => "\\int",
        Some('∬') => "\\iint",
        Some('∭') => "\\iiint",
        Some('∮') => "\\oint",
        Some('⋃') => "\\bigcup",
        Some('⋂') => "\\bigcap",
        Some('⋁') => "\\bigvee",
        Some('⋀') => "\\bigwedge",
        Some('⨁') => "\\bigoplus",
        Some('⨂') => "\\bigotimes",
        Some('⨀') => "\\bigodot",
        Some('⨄') => "\\biguplus",
        Some('⨆') => "\\bigsqcup",
        _ => "\\int",
    }
}

/// Maps a combining accent character to its TeX command, defaulting to a wide hat.
#[allow(clippy::match_same_arms)]
fn accent_command(chr: char) -> &'static str {
    match chr {
        '\u{0300}' => "\\grave",
        '\u{0301}' => "\\acute",
        '\u{0302}' => "\\widehat",
        '\u{0303}' => "\\widetilde",
        '\u{0304}' => "\\bar",
        '\u{0305}' => "\\overline",
        '\u{0306}' => "\\breve",
        '\u{0307}' => "\\dot",
        '\u{0308}' => "\\ddot",
        '\u{030A}' => "\\mathring",
        '\u{030C}' => "\\check",
        '\u{20D7}' => "\\vec",
        _ => "\\widehat",
    }
}

/// Recognized math function names rendered as TeX control words.
const KNOWN_FUNCTIONS: &[&str] = &[
    "sin", "cos", "tan", "cot", "sec", "csc", "sinh", "cosh", "tanh", "coth", "arcsin", "arccos",
    "arctan", "log", "ln", "lg", "exp", "lim", "max", "min", "det", "gcd", "deg", "dim", "hom",
    "ker", "arg", "sup", "inf", "liminf", "limsup",
];

/// Maps a recognized function name to its TeX command, leaving unknown names verbatim.
fn map_function(name: &str) -> String {
    let trimmed = name.trim();
    if KNOWN_FUNCTIONS.contains(&trimmed) {
        format!("\\{trimmed}")
    } else {
        trimmed.to_owned()
    }
}

/// Maps a math run's text, translating a small set of common symbols to TeX commands and passing
/// everything else through. Exotic symbols outside this set are emitted literally.
fn map_math_text(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if let Some(command) = math_symbol(ch) {
            out.push_str(command);
        } else {
            out.push(ch);
        }
    }
    out
}

fn math_symbol(ch: char) -> Option<&'static str> {
    let command = match ch {
        'α' => "\\alpha ",
        'β' => "\\beta ",
        'γ' => "\\gamma ",
        'δ' => "\\delta ",
        'ε' => "\\varepsilon ",
        'ζ' => "\\zeta ",
        'η' => "\\eta ",
        'θ' => "\\theta ",
        'λ' => "\\lambda ",
        'μ' => "\\mu ",
        'π' => "\\pi ",
        'ρ' => "\\rho ",
        'σ' => "\\sigma ",
        'τ' => "\\tau ",
        'φ' => "\\varphi ",
        'χ' => "\\chi ",
        'ψ' => "\\psi ",
        'ω' => "\\omega ",
        'Γ' => "\\Gamma ",
        'Δ' => "\\Delta ",
        'Θ' => "\\Theta ",
        'Λ' => "\\Lambda ",
        'Π' => "\\Pi ",
        'Σ' => "\\Sigma ",
        'Φ' => "\\Phi ",
        'Ψ' => "\\Psi ",
        'Ω' => "\\Omega ",
        '∞' => "\\infty ",
        '×' => "\\times ",
        '÷' => "\\div ",
        '±' => "\\pm ",
        '∓' => "\\mp ",
        '⋅' => "\\cdot ",
        '≤' => "\\leq ",
        '≥' => "\\geq ",
        '≠' => "\\neq ",
        '≈' => "\\approx ",
        '≡' => "\\equiv ",
        '∈' => "\\in ",
        '∉' => "\\notin ",
        '⊂' => "\\subset ",
        '⊆' => "\\subseteq ",
        '∪' => "\\cup ",
        '∩' => "\\cap ",
        '→' => "\\to ",
        '⇒' => "\\Rightarrow ",
        '⇔' => "\\Leftrightarrow ",
        '∂' => "\\partial ",
        '∇' => "\\nabla ",
        '∀' => "\\forall ",
        '∃' => "\\exists ",
        '∅' => "\\emptyset ",
        _ => return None,
    };
    Some(command)
}
