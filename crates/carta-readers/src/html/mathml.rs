//! Presentation MathML → TeX rendering for the `<math>` element.
//!
//! The element tree is walked into a TeX string: token elements (`<mi>`, `<mn>`, `<mo>`) map to
//! their literal or symbolic form, and layout elements (`<msup>`, `<mfrac>`, `<msqrt>`, …) wrap their
//! rendered children in the matching TeX construct. An operator that reads as a binary or relation
//! symbol is spaced from its neighbors; large operators, punctuation, and fences sit tight.

use super::tree::{Element, Node, attr_value, collect_text};

/// Render a `<math>` element's presentation MathML to a TeX string.
pub(super) fn to_tex(math: &Element) -> String {
    render_row(&math.children)
}

/// Render a sequence of nodes as an inline row, ignoring inter-element whitespace and comments, then
/// trim the surrounding spacing an edge operator would otherwise leave.
fn render_row(nodes: &[Node]) -> String {
    let pieces: Vec<String> = nodes
        .iter()
        .filter_map(|node| match node {
            Node::Element(element) => {
                let rendered = render(element);
                (!rendered.is_empty()).then_some(rendered)
            }
            _ => None,
        })
        .collect();
    join_tokens(&pieces).trim().to_string()
}

/// Concatenate rendered row pieces, inserting a separating space wherever the left piece ends with a
/// control word (`\` followed by letters) and the right piece begins with a letter or digit, so a
/// command does not swallow the token that follows it (`\int f`, not `\intf`).
fn join_tokens(pieces: &[String]) -> String {
    let mut out = String::new();
    for piece in pieces {
        if ends_with_control_word(&out)
            && piece
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric())
        {
            out.push(' ');
        }
        out.push_str(piece);
    }
    out
}

/// Whether `s` ends with a TeX control word: a run of ASCII letters immediately preceded by a
/// backslash.
fn ends_with_control_word(s: &str) -> bool {
    let head = s.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    head.len() < s.len() && head.ends_with('\\')
}

fn render(e: &Element) -> String {
    match e.name.as_str() {
        "mi" => render_ident(collect_text(e).trim()),
        "mn" => collect_text(e).trim().to_string(),
        "mo" => render_operator(collect_text(e).trim()),
        "mtext" => format!("\\text{{{}}}", collect_text(e).trim()),
        "mspace" => String::new(),
        "msup" => render_script(e, '^'),
        "msub" => render_script(e, '_'),
        "msubsup" => render_subsup(e),
        "mfrac" => render_binary(e, "\\frac"),
        "msqrt" => format!("\\sqrt{{{}}}", render_row(&e.children)),
        "mroot" => render_root(e),
        "mover" => render_over(e),
        "munder" => render_under(e),
        "munderover" => render_underover(e),
        "mfenced" => render_fenced(e),
        "semantics" => render_semantics(e),
        // A grouping or presentational wrapper carries no structure of its own: render its content.
        _ => render_row(&e.children),
    }
}

/// The `index`-th element child, skipping text and comment nodes.
fn nth_element(e: &Element, index: usize) -> Option<&Element> {
    e.children
        .iter()
        .filter_map(|node| match node {
            Node::Element(element) => Some(element),
            _ => None,
        })
        .nth(index)
}

fn rendered_child(e: &Element, index: usize) -> String {
    nth_element(e, index).map(render).unwrap_or_default()
}

/// A single-script element (`<msup>`/`<msub>`): base plus one script in braces.
fn render_script(e: &Element, marker: char) -> String {
    let base = rendered_child(e, 0);
    let script = rendered_child(e, 1);
    format!("{}{}{{{}}}", brace_base(&base), marker, script)
}

/// `<msubsup>`: base with both a subscript and a superscript.
fn render_subsup(e: &Element) -> String {
    let base = rendered_child(e, 0);
    let sub = rendered_child(e, 1);
    let sup = rendered_child(e, 2);
    format!("{}_{{{}}}^{{{}}}", brace_base(&base), sub, sup)
}

/// A two-argument construct written `cmd{first}{second}`, e.g. `<mfrac>` → `\frac`.
fn render_binary(e: &Element, command: &str) -> String {
    let first = rendered_child(e, 0);
    let second = rendered_child(e, 1);
    format!("{command}{{{first}}}{{{second}}}")
}

/// `<mroot>`: base under a radical with an explicit index.
fn render_root(e: &Element) -> String {
    let base = rendered_child(e, 0);
    let index = rendered_child(e, 1);
    format!("\\sqrt[{index}]{{{base}}}")
}

/// `<mover>`: a base with an overscript accent, mapped to the matching accent command.
fn render_over(e: &Element) -> String {
    let base = rendered_child(e, 0);
    let accent = nth_element(e, 1)
        .map(|c| collect_text(c).trim().to_string())
        .unwrap_or_default();
    format!("{}{{{}}}", accent_command(&accent), base)
}

/// `<munder>`: an under-script. A large operator or limit-like function carries its script with
/// `\limits`; anything else uses `\underset`.
fn render_under(e: &Element) -> String {
    let base = rendered_child(e, 0);
    let under = rendered_child(e, 1);
    if base.starts_with('\\') {
        format!("{base}\\limits_{{{under}}}")
    } else {
        format!("\\underset{{{under}}}{{{base}}}")
    }
}

/// `<munderover>`: both an under-script and an over-script on the base.
fn render_underover(e: &Element) -> String {
    let base = rendered_child(e, 0);
    let under = rendered_child(e, 1);
    let over = rendered_child(e, 2);
    if base.starts_with('\\') {
        format!("{base}\\limits_{{{under}}}^{{{over}}}")
    } else {
        format!("\\overset{{{over}}}{{\\underset{{{under}}}{{{base}}}}}")
    }
}

/// `<mfenced>`: children wrapped in delimiters, defaulting to parentheses with comma separators.
fn render_fenced(e: &Element) -> String {
    let open = attr_value(e, "open").unwrap_or_else(|| "(".to_string());
    let close = attr_value(e, "close").unwrap_or_else(|| ")".to_string());
    let separators = attr_value(e, "separators").unwrap_or_else(|| ",".to_string());
    let separator = separators.chars().next().unwrap_or(',').to_string();
    let parts: Vec<String> = e
        .children
        .iter()
        .filter_map(|node| match node {
            Node::Element(element) => Some(render(element)),
            _ => None,
        })
        .collect();
    format!("{open}{}{close}", parts.join(&separator))
}

/// `<semantics>`: render the presentation child, dropping any annotation payload.
fn render_semantics(e: &Element) -> String {
    for node in &e.children {
        if let Node::Element(element) = node {
            if element.name == "annotation" || element.name == "annotation-xml" {
                continue;
            }
            return render(element);
        }
    }
    String::new()
}

/// A script base is braced unless it is a single character, so `x^{2}` stays bare while a compound
/// base like `a + b` is grouped.
fn brace_base(base: &str) -> String {
    if base.chars().count() == 1 {
        base.to_string()
    } else {
        format!("{{{base}}}")
    }
}

/// Map an identifier to its TeX form: a Greek letter to its command, a known function name to its
/// control word, and anything else to the literal text.
fn render_ident(ident: &str) -> String {
    if ident.is_empty() {
        return String::new();
    }
    if let Some(command) = greek(ident) {
        return command.to_string();
    }
    if is_function(ident) {
        return format!("\\{ident}");
    }
    ident.to_string()
}

fn is_function(name: &str) -> bool {
    matches!(
        name,
        "sin"
            | "cos"
            | "tan"
            | "cot"
            | "sec"
            | "csc"
            | "sinh"
            | "cosh"
            | "tanh"
            | "coth"
            | "arcsin"
            | "arccos"
            | "arctan"
            | "log"
            | "ln"
            | "lg"
            | "exp"
            | "lim"
            | "limsup"
            | "liminf"
            | "max"
            | "min"
            | "sup"
            | "inf"
            | "det"
            | "dim"
            | "gcd"
            | "hom"
            | "ker"
            | "arg"
            | "deg"
            | "Pr"
    )
}

fn greek(ident: &str) -> Option<&'static str> {
    Some(match ident {
        "\u{3b1}" => "\\alpha",
        "\u{3b2}" => "\\beta",
        "\u{3b3}" => "\\gamma",
        "\u{3b4}" => "\\delta",
        "\u{3b5}" => "\\epsilon",
        "\u{3b6}" => "\\zeta",
        "\u{3b7}" => "\\eta",
        "\u{3b8}" => "\\theta",
        "\u{3b9}" => "\\iota",
        "\u{3ba}" => "\\kappa",
        "\u{3bb}" => "\\lambda",
        "\u{3bc}" => "\\mu",
        "\u{3bd}" => "\\nu",
        "\u{3be}" => "\\xi",
        "\u{3c0}" => "\\pi",
        "\u{3c1}" => "\\rho",
        "\u{3c3}" => "\\sigma",
        "\u{3c4}" => "\\tau",
        "\u{3c5}" => "\\upsilon",
        "\u{3c6}" => "\\phi",
        "\u{3c7}" => "\\chi",
        "\u{3c8}" => "\\psi",
        "\u{3c9}" => "\\omega",
        "\u{393}" => "\\Gamma",
        "\u{394}" => "\\Delta",
        "\u{398}" => "\\Theta",
        "\u{39b}" => "\\Lambda",
        "\u{39e}" => "\\Xi",
        "\u{3a0}" => "\\Pi",
        "\u{3a3}" => "\\Sigma",
        "\u{3a6}" => "\\Phi",
        "\u{3a8}" => "\\Psi",
        "\u{3a9}" => "\\Omega",
        _ => return None,
    })
}

/// Map an accent character to its TeX accent command, defaulting to an overline.
fn accent_command(accent: &str) -> &'static str {
    match accent {
        "^" | "\u{302}" => "\\hat",
        "~" | "\u{303}" => "\\tilde",
        "\u{2192}" | "\u{20d7}" => "\\vec",
        "." | "\u{2d9}" | "\u{307}" => "\\dot",
        _ => "\\overline",
    }
}

/// The four Unicode invisible operators (function application, invisible times, and separators)
/// carry no printed form.
fn is_invisible(op: &str) -> bool {
    matches!(op, "\u{2061}" | "\u{2062}" | "\u{2063}" | "\u{2064}")
}

/// Render an operator. An empty or invisible operator vanishes; a function name spelled as an
/// operator takes its control word; a binary or relation symbol is surrounded by spaces so it sits
/// apart from its operands; large operators, arrows, punctuation, and fences stay tight (the row
/// join reintroduces any space a following command needs).
fn render_operator(op: &str) -> String {
    if op.is_empty() || is_invisible(op) {
        return String::new();
    }
    if is_function(op) {
        return format!("\\{op}");
    }
    let (tex, spaced) = operator_tex(op);
    if spaced { format!(" {tex} ") } else { tex }
}

fn operator_tex(op: &str) -> (String, bool) {
    match op {
        "+" => ("+".into(), true),
        "-" | "\u{2212}" => ("-".into(), true),
        "=" => ("=".into(), true),
        "<" => ("<".into(), true),
        ">" => (">".into(), true),
        "\u{d7}" => ("\\times".into(), true),
        "\u{f7}" => ("\\div".into(), true),
        "\u{b7}" => ("\\cdot".into(), true),
        "\u{2217}" => ("\\ast".into(), true),
        "\u{2264}" => ("\\leq".into(), true),
        "\u{2265}" => ("\\geq".into(), true),
        "\u{2260}" => ("\\neq".into(), true),
        "\u{2248}" => ("\\approx".into(), true),
        "\u{2261}" => ("\\equiv".into(), true),
        "\u{b1}" => ("\\pm".into(), true),
        "\u{2213}" => ("\\mp".into(), true),
        "\u{2208}" => ("\\in".into(), true),
        "\u{2209}" => ("\\notin".into(), true),
        "\u{2192}" => ("\\rightarrow".into(), false),
        "\u{2190}" => ("\\leftarrow".into(), false),
        "\u{2194}" => ("\\leftrightarrow".into(), false),
        "\u{21d2}" => ("\\Rightarrow".into(), false),
        "\u{21d0}" => ("\\Leftarrow".into(), false),
        "\u{21d4}" => ("\\Leftrightarrow".into(), false),
        "\u{21a6}" => ("\\mapsto".into(), false),
        "\u{2211}" => ("\\sum".into(), false),
        "\u{220f}" => ("\\prod".into(), false),
        "\u{222b}" => ("\\int".into(), false),
        "\u{222e}" => ("\\oint".into(), false),
        "\u{221a}" => ("\\sqrt{}".into(), false),
        "\u{221e}" => ("\\infty".into(), false),
        "\u{2032}" => ("'".into(), false),
        _ => (op.into(), false),
    }
}
