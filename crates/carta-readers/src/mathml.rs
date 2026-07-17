//! Presentation MathML → TeX rendering, shared by every reader that carries an embedded `<math>`
//! tree.
//!
//! The element tree is walked into a TeX string: token elements (`<mi>`, `<mn>`, `<mo>`) map to
//! their literal or symbolic form, and layout elements (`<msup>`, `<mfrac>`, `<msqrt>`, …) wrap their
//! rendered children in the matching TeX construct. An operator that reads as a binary or relation
//! symbol is spaced from its neighbors; large operators, punctuation, and fences sit tight.
//!
//! The walk is written against [`MathTree`], a minimal read-only view of an element, so the same
//! renderer serves the different element trees the container readers build.

/// A read-only view of a MathML element: enough of an element's shape to render it, abstracted over
/// the concrete tree a given reader parsed into.
pub(crate) trait MathTree: Sized {
    /// The element's local tag name, with any namespace prefix stripped.
    fn tag(&self) -> &str;
    /// The value of the attribute whose local name is `key`.
    fn attribute(&self, key: &str) -> Option<String>;
    /// The concatenated character data of this element and its descendants.
    fn inner_text(&self) -> String;
    /// The child elements, in order.
    fn element_children(&self) -> Vec<&Self>;
    /// The `index`-th child element, resolved without materializing the whole child list.
    fn nth_element_child(&self, index: usize) -> Option<&Self>;
}

#[cfg(any(feature = "docx", feature = "epub", feature = "odt"))]
impl MathTree for crate::xml::Element {
    fn tag(&self) -> &str {
        crate::xml::local_name(&self.name)
    }
    fn attribute(&self, key: &str) -> Option<String> {
        self.attr(key).map(str::to_owned)
    }
    fn inner_text(&self) -> String {
        self.text()
    }
    fn element_children(&self) -> Vec<&Self> {
        self.elements().collect()
    }
    fn nth_element_child(&self, index: usize) -> Option<&Self> {
        self.elements().nth(index)
    }
}

/// Render a `<math>` element's presentation MathML to a TeX string.
pub(crate) fn to_tex<T: MathTree>(math: &T) -> String {
    render_row(&math.element_children())
}

/// Render a sequence of element children as an inline row, then trim the surrounding spacing an edge
/// operator would otherwise leave.
fn render_row<T: MathTree>(elements: &[&T]) -> String {
    let mut pieces: Vec<String> = Vec::new();
    let mut index = 0;
    while index < elements.len() {
        // A matrix wrapped in a fence — an open operator, an `<mtable>`, then a close operator —
        // reads as one delimited matrix rather than three loose tokens.
        if let (Some(open), Some(table), Some(close)) = (
            elements.get(index),
            elements.get(index + 1),
            elements.get(index + 2),
        ) && let Some(rendered) = matrix_fence(*open, *table, *close)
        {
            pieces.push(rendered);
            index += 3;
            continue;
        }
        if let Some(element) = elements.get(index) {
            let rendered = render(*element);
            if !rendered.is_empty() {
                pieces.push(rendered);
            }
        }
        index += 1;
    }
    let row = join_tokens(&pieces);
    let trimmed = row.trim();
    // Trimming an edge operator's spacing can strip the space that a four-mu control space `\ ` needs,
    // leaving a dangling backslash; a trailing lone backslash can only be that command, so give it
    // back its space.
    if ends_with_lone_backslash(trimmed) {
        format!("{trimmed} ")
    } else {
        trimmed.to_string()
    }
}

/// Whether `s` ends with an odd run of backslashes, so its final backslash stands alone rather than
/// closing an escaped pair.
fn ends_with_lone_backslash(s: &str) -> bool {
    s.chars().rev().take_while(|&c| c == '\\').count() % 2 == 1
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

fn render<T: MathTree>(e: &T) -> String {
    match e.tag() {
        "mi" => render_ident(e.inner_text().trim()),
        "mn" => e.inner_text().trim().to_string(),
        "mo" => render_operator(e.inner_text().trim()),
        "mtext" => format!("\\text{{{}}}", escape_text(e.inner_text().trim())),
        "ms" => render_string(e),
        "mspace" => render_space(e),
        "msup" => render_script(e, '^'),
        "msub" => render_script(e, '_'),
        "msubsup" => render_subsup(e),
        "mfrac" => render_binary(e, "\\frac"),
        "msqrt" => format!("\\sqrt{{{}}}", render_row(&e.element_children())),
        "mroot" => render_root(e),
        "mover" => render_over(e),
        "munder" => render_under(e),
        "munderover" => render_underover(e),
        "mfenced" => render_fenced(e),
        "mtable" => render_mtable(e, "matrix"),
        "mmultiscripts" => render_mmultiscripts(e),
        "mphantom" => format!("\\phantom{{{}}}", render_row(&e.element_children())),
        "menclose" => render_menclose(e),
        "semantics" => render_semantics(e),
        // A grouping or presentational wrapper carries no structure of its own: render its content.
        _ => render_row(&e.element_children()),
    }
}

/// The `index`-th element child.
fn nth_child<T: MathTree>(e: &T, index: usize) -> Option<&T> {
    e.nth_element_child(index)
}

fn rendered_child<T: MathTree>(e: &T, index: usize) -> String {
    nth_child(e, index).map(render).unwrap_or_default()
}

/// A single-script element (`<msup>`/`<msub>`): base plus one script in braces.
fn render_script<T: MathTree>(e: &T, marker: char) -> String {
    let base = rendered_child(e, 0);
    let script = rendered_child(e, 1);
    format!("{}{}{{{}}}", brace_base(&base), marker, script)
}

/// `<msubsup>`: base with both a subscript and a superscript.
fn render_subsup<T: MathTree>(e: &T) -> String {
    let base = rendered_child(e, 0);
    let sub = rendered_child(e, 1);
    let sup = rendered_child(e, 2);
    format!("{}_{{{}}}^{{{}}}", brace_base(&base), sub, sup)
}

/// A two-argument construct written `cmd{first}{second}`, e.g. `<mfrac>` → `\frac`.
fn render_binary<T: MathTree>(e: &T, command: &str) -> String {
    let first = rendered_child(e, 0);
    let second = rendered_child(e, 1);
    format!("{command}{{{first}}}{{{second}}}")
}

/// `<mroot>`: base under a radical with an explicit index.
fn render_root<T: MathTree>(e: &T) -> String {
    let base = rendered_child(e, 0);
    let index = rendered_child(e, 1);
    format!("\\sqrt[{index}]{{{base}}}")
}

/// `<mover>`: a base with an overscript accent, mapped to the matching accent command.
fn render_over<T: MathTree>(e: &T) -> String {
    let base = rendered_child(e, 0);
    let accent = nth_child(e, 1)
        .map(|c| c.inner_text().trim().to_string())
        .unwrap_or_default();
    format!("{}{{{}}}", accent_command(&accent), base)
}

/// `<munder>`: an under-script. A large operator or limit-like function carries its script with
/// `\limits`; anything else uses `\underset`.
fn render_under<T: MathTree>(e: &T) -> String {
    let base = rendered_child(e, 0);
    let under = rendered_child(e, 1);
    if base.starts_with('\\') {
        format!("{base}\\limits_{{{under}}}")
    } else {
        format!("\\underset{{{under}}}{{{base}}}")
    }
}

/// `<munderover>`: both an under-script and an over-script on the base.
fn render_underover<T: MathTree>(e: &T) -> String {
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
fn render_fenced<T: MathTree>(e: &T) -> String {
    let open = e.attribute("open").unwrap_or_else(|| "(".to_string());
    let close = e.attribute("close").unwrap_or_else(|| ")".to_string());
    let separators = e.attribute("separators").unwrap_or_else(|| ",".to_string());
    let separator = separators.chars().next().unwrap_or(',').to_string();
    let parts: Vec<String> = e.element_children().iter().map(|c| render(*c)).collect();
    format!("{open}{}{close}", parts.join(&separator))
}

/// The named matrix environment a fence pair selects, or `None` for a fence that keeps an explicit
/// `\left`…`\right` wrapping instead.
fn matrix_env(open: &str, close: &str) -> Option<&'static str> {
    match (open, close) {
        ("(", ")") => Some("pmatrix"),
        ("[", "]") => Some("bmatrix"),
        ("{", "}") => Some("Bmatrix"),
        _ => None,
    }
}

/// The `\left`/`\right` operand a stretchy bar fence maps to, for a delimiter pair with no dedicated
/// matrix environment.
fn left_right_delim(op: &str) -> Option<&'static str> {
    match op {
        "|" => Some("|"),
        "\u{2016}" => Some("\\|"),
        _ => None,
    }
}

/// An open operator, a table, and a close operator taken together as a delimited matrix: a
/// recognized bracket pair becomes the matching matrix environment, and a stretchy bar pair wraps a
/// plain matrix in `\left`…`\right`.
fn matrix_fence<T: MathTree>(open: &T, table: &T, close: &T) -> Option<String> {
    if open.tag() != "mo" || table.tag() != "mtable" || close.tag() != "mo" {
        return None;
    }
    let open_text = open.inner_text();
    let close_text = close.inner_text();
    let (open_delim, close_delim) = (open_text.trim(), close_text.trim());
    if let Some(env) = matrix_env(open_delim, close_delim) {
        return Some(render_mtable(table, env));
    }
    if let (Some(left), Some(right)) = (left_right_delim(open_delim), left_right_delim(close_delim))
    {
        return Some(format!(
            "\\left{left} {} \\right{right}",
            render_mtable(table, "matrix")
        ));
    }
    None
}

/// `<mtable>`: rows of cells laid out as a TeX matrix, cells separated by `&` and rows by `\\`. Every
/// row is padded to the widest so the columns line up, and a multi-token cell is braced so its
/// content reads as one grid entry.
fn render_mtable<T: MathTree>(e: &T, env: &str) -> String {
    let rows: Vec<Vec<String>> = e
        .element_children()
        .into_iter()
        .filter(|row| row.tag() == "mtr")
        .map(|row| {
            row.element_children()
                .into_iter()
                .filter(|cell| cell.tag() == "mtd")
                .map(|cell| matrix_cell(&render_row(&cell.element_children())))
                .collect()
        })
        .collect();
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    let lines: Vec<String> = rows
        .into_iter()
        .map(|mut cells| {
            cells.resize(width, String::new());
            cells.join(" & ")
        })
        .collect();
    format!(
        "\\begin{{{env}}}\n{}\n\\end{{{env}}}",
        lines.join(" \\\\\n")
    )
}

/// A matrix cell: a lone token sits bare, a compound expression is braced so it reads as one entry,
/// and an empty cell contributes nothing.
fn matrix_cell(content: &str) -> String {
    match content.chars().count() {
        0 => String::new(),
        1 => content.to_string(),
        _ => format!("{{{content}}}"),
    }
}

/// `<mmultiscripts>`: a base carrying post-scripts and, after an `<mprescripts/>` marker, pre-scripts.
/// Each slot pair is emitted as `_{sub}^{sup}` with a `<none/>` slot left empty, behind a leading
/// empty group so any pre-scripts have a nucleus to attach to.
fn render_mmultiscripts<T: MathTree>(e: &T) -> String {
    let children = e.element_children();
    let mut iter = children.into_iter();
    let base = iter
        .next()
        .map(|element| render(element))
        .unwrap_or_default();
    let mut pre = String::new();
    let mut post = String::new();
    let mut in_pre = false;
    while let Some(sub_element) = iter.next() {
        if sub_element.tag() == "mprescripts" {
            in_pre = true;
            continue;
        }
        let target_pre = in_pre;
        let sub = script_token(sub_element);
        let sup = match iter.next() {
            Some(element) if element.tag() == "mprescripts" => {
                in_pre = true;
                String::new()
            }
            Some(element) => script_token(element),
            None => String::new(),
        };
        let pair = format!("_{{{sub}}}^{{{sup}}}");
        if target_pre {
            pre.push_str(&pair);
        } else {
            post.push_str(&pair);
        }
    }
    format!("{{}}{pre}{}{post}", brace_base(&base))
}

/// A single multiscript slot: an explicit empty (`<none/>`) contributes nothing, anything else its
/// rendered form.
fn script_token<T: MathTree>(e: &T) -> String {
    if e.tag() == "none" {
        String::new()
    } else {
        render(e)
    }
}

/// `<menclose>`: content wrapped in the TeX command its `notation` denotes — a boxed frame or a
/// cancel line — or left bare for a notation with no TeX equivalent.
fn render_menclose<T: MathTree>(e: &T) -> String {
    let inner = render_row(&e.element_children());
    match enclose_command(&e.attribute("notation").unwrap_or_default()) {
        Some(command) => format!("{command}{{{inner}}}"),
        None => inner,
    }
}

/// The TeX command an `menclose` notation set maps to: diagonal strikes become cancels (up, down, or
/// both crossed), a box becomes `\boxed`, and anything else has no command.
fn enclose_command(notation: &str) -> Option<&'static str> {
    let up = notation
        .split_whitespace()
        .any(|token| token == "updiagonalstrike");
    let down = notation
        .split_whitespace()
        .any(|token| token == "downdiagonalstrike");
    match (up, down) {
        (true, true) => Some("\\xcancel"),
        (true, false) => Some("\\cancel"),
        (false, true) => Some("\\bcancel"),
        (false, false) => notation
            .split_whitespace()
            .any(|token| token == "box")
            .then_some("\\boxed"),
    }
}

/// `<semantics>`: render the presentation child, dropping any annotation payload.
fn render_semantics<T: MathTree>(e: &T) -> String {
    for element in e.element_children() {
        if element.tag() == "annotation" || element.tag() == "annotation-xml" {
            continue;
        }
        return render(element);
    }
    String::new()
}

/// `<ms>`: a string literal set inside a `\text{...}` box between quotation marks. The `lquote` and
/// `rquote` attributes supply the marks, defaulting to typographic double quotes, and the literal
/// text has its LaTeX specials escaped.
fn render_string<T: MathTree>(e: &T) -> String {
    let open = e
        .attribute("lquote")
        .unwrap_or_else(|| "\u{201c}".to_string());
    let close = e
        .attribute("rquote")
        .unwrap_or_else(|| "\u{201d}".to_string());
    format!(
        "\\text{{{open}{}{close}}}",
        escape_text(e.inner_text().trim())
    )
}

/// Escape text bound for a TeX text box (`\text{...}`): the characters LaTeX reads as control syntax
/// take their text-mode escapes. The three that expand to a control word are held apart from a
/// following letter or digit so the command does not absorb it.
fn escape_text(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        match ch {
            '%' => out.push_str("\\%"),
            '&' => out.push_str("\\&"),
            '_' => out.push_str("\\_"),
            '#' => out.push_str("\\#"),
            '$' => out.push_str("\\$"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '~' => out.push_str("\\textasciitilde"),
            '^' => out.push_str("\\textasciicircum"),
            '\\' => out.push_str("\\textbackslash"),
            other => {
                if other.is_ascii_alphanumeric() && ends_with_control_word(&out) {
                    out.push(' ');
                }
                out.push(other);
            }
        }
    }
    out
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
        "\u{b7}" | "\u{22c5}" => ("\\cdot".into(), true),
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
        "\u{2282}" => ("\\subset".into(), true),
        "\u{2283}" => ("\\supset".into(), true),
        "\u{2286}" => ("\\subseteq".into(), true),
        "\u{2287}" => ("\\supseteq".into(), true),
        "\u{228a}" => ("\\subsetneq".into(), true),
        "\u{228b}" => ("\\supsetneq".into(), true),
        "\u{220b}" => ("\\ni".into(), true),
        "\u{2245}" => ("\\cong".into(), true),
        "\u{221d}" => ("\\propto".into(), true),
        "\u{2225}" => ("\\parallel".into(), true),
        "\u{2226}" => ("\\nparallel".into(), true),
        "\u{2223}" => ("\\mid".into(), true),
        "\u{2224}" => ("\\nmid".into(), true),
        "\u{226a}" => ("\\ll".into(), true),
        "\u{226b}" => ("\\gg".into(), true),
        "\u{227a}" => ("\\prec".into(), true),
        "\u{227b}" => ("\\succ".into(), true),
        "\u{226c}" => ("\\between".into(), true),
        "\u{224d}" => ("\\asymp".into(), true),
        "\u{2250}" => ("\\doteq".into(), true),
        "\u{2252}" => ("\\fallingdotseq".into(), true),
        "\u{2253}" => ("\\risingdotseq".into(), true),
        "\u{2227}" => ("\\land".into(), true),
        "\u{2228}" => ("\\vee".into(), true),
        "\u{222a}" => ("\\cup".into(), true),
        "\u{2229}" => ("\\cap".into(), true),
        "\u{2295}" => ("\\oplus".into(), true),
        "\u{2296}" => ("\\ominus".into(), true),
        "\u{2297}" => ("\\otimes".into(), true),
        "\u{2298}" => ("\\oslash".into(), true),
        "\u{2299}" => ("\\odot".into(), true),
        "\u{2218}" => ("\\circ".into(), true),
        "\u{2219}" => ("\\bullet".into(), true),
        "\u{2216}" => ("\\smallsetminus".into(), true),
        "\u{22c6}" => ("\\star".into(), true),
        "\u{2020}" => ("\\dagger".into(), true),
        "\u{2021}" => ("\\ddagger".into(), true),
        "\u{2200}" => ("\\forall".into(), false),
        "\u{2203}" => ("\\exists".into(), false),
        "\u{2202}" => ("\\partial".into(), false),
        "\u{2207}" => ("\\nabla".into(), false),
        "\u{2205}" => ("\\varnothing".into(), false),
        "\u{22a5}" => ("\\bot".into(), false),
        "\u{2220}" => ("\\angle".into(), false),
        "\u{ac}" => ("\\neg".into(), false),
        "\u{2113}" => ("\\ell".into(), false),
        "\u{2118}" => ("\\wp".into(), false),
        "\u{2135}" => ("\\aleph".into(), false),
        "\u{25a1}" => ("\\square".into(), false),
        "\u{2662}" => ("\\diamondsuit".into(), false),
        "\u{2663}" => ("\\clubsuit".into(), false),
        "\u{2660}" => ("\\spadesuit".into(), false),
        "\u{2661}" => ("\\heartsuit".into(), false),
        "\u{22c0}" => ("\\bigwedge".into(), false),
        "\u{22c1}" => ("\\bigvee".into(), false),
        "\u{22c2}" => ("\\bigcap".into(), false),
        "\u{22c3}" => ("\\bigcup".into(), false),
        "\u{222c}" => ("\\iint".into(), false),
        "\u{222d}" => ("\\iiint".into(), false),
        "\u{2210}" => ("\\coprod".into(), false),
        "\u{2a00}" => ("\\bigodot".into(), false),
        "\u{2a01}" => ("\\bigoplus".into(), false),
        "\u{2a02}" => ("\\bigotimes".into(), false),
        "\u{2a04}" => ("\\biguplus".into(), false),
        "\u{2a06}" => ("\\bigsqcup".into(), false),
        _ => (op.into(), false),
    }
}

/// `<mspace>`: a horizontal gap rendered as the TeX spacing command its `width` selects, followed by a
/// separating space so the gap stays apart from the token after it. A named math-space keyword or an
/// `em` length is honored; a width in any other unit yields no command, leaving just the separator.
fn render_space<T: MathTree>(e: &T) -> String {
    let mu = e.attribute("width").and_then(|w| space_mu(&w)).unwrap_or(0);
    format!("{} ", space_command(mu))
}

/// The width of an `<mspace>` in math units: a named math space, or an `em` length scaled at eighteen
/// mu to the em with ties rounded to even. `None` for a width given in any other form.
fn space_mu(width: &str) -> Option<i32> {
    if let Some(mu) = named_space_mu(width) {
        return Some(mu);
    }
    let em = width.strip_suffix("em")?;
    if em.starts_with('+') {
        return None;
    }
    let value: f64 = em.parse().ok()?;
    if !value.is_finite() {
        return None;
    }
    // The measure is finite; the saturating cast bounds an extreme scaled value into `i32`.
    #[allow(clippy::cast_possible_truncation)]
    let mu = (value * 18.0).round_ties_even() as i32;
    Some(mu)
}

/// The math-unit width of a named MathML space keyword, thin through very-very-thick and their
/// negatives, each one mu apart.
fn named_space_mu(name: &str) -> Option<i32> {
    Some(match name {
        "veryverythinmathspace" => 1,
        "verythinmathspace" => 2,
        "thinmathspace" => 3,
        "mediummathspace" => 4,
        "thickmathspace" => 5,
        "verythickmathspace" => 6,
        "veryverythickmathspace" => 7,
        "negativeveryverythinmathspace" => -1,
        "negativeverythinmathspace" => -2,
        "negativethinmathspace" => -3,
        "negativemediummathspace" => -4,
        "negativethickmathspace" => -5,
        "negativeverythickmathspace" => -6,
        "negativeveryverythickmathspace" => -7,
        _ => return None,
    })
}

/// The TeX spacing command for a width in math units: the short control-symbol spaces where one
/// exists, `\quad`/`\qquad` at the em and double-em, an empty command at zero width, and an explicit
/// `\mspace` for every other amount. The bare backslash for four mu becomes the control space `\ `
/// once the caller appends its separator, and is kept as a control space at a row's edge.
fn space_command(mu: i32) -> String {
    match mu {
        0 => String::new(),
        3 => "\\,".to_string(),
        4 => "\\".to_string(),
        5 => "\\;".to_string(),
        -3 => "\\!".to_string(),
        18 => "\\quad".to_string(),
        36 => "\\qquad".to_string(),
        other => format!("\\mspace{{{other}mu}}"),
    }
}
