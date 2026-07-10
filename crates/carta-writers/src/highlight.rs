//! Helpers shared by the writers that colorize code blocks (the HTML family and LaTeX): recognizing
//! the line-numbering class, reading the starting line number, splitting an unclassified block into
//! lines, and projecting a color theme onto the preamble each standalone target needs — the CSS the
//! HTML family embeds and the token macros LaTeX defines. Token escaping and per-token markup are
//! format-specific and stay with each writer.

use std::fmt::Write as _;

use carta_ast::{Attr, Text};
use carta_highlight::{SourceLine, Theme, Token, TokenKind, TokenStyle};

/// Whether a class requests per-line numbering on a code block.
pub(crate) fn is_number_lines_class(class: &Text) -> bool {
    matches!(class.as_str(), "numberLines" | "number-lines")
}

/// The first line's number: the `startFrom` key parsed as an integer, or 1 when absent or unparsable.
pub(crate) fn start_line(attr: &Attr) -> i64 {
    attr.attributes
        .iter()
        .find(|(key, _)| key.as_str() == "startFrom")
        .and_then(|(_, value)| value.as_str().parse::<i64>().ok())
        .unwrap_or(1)
}

/// Split a code block's text into lines the way the tokenizer does, treating each as a single
/// unclassified run. Used when a block gets the structured scaffolding but names no known language,
/// so every line is one plain token without any color.
pub(crate) fn plain_source_lines(text: &str) -> Vec<SourceLine> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<&str> = text.split('\n').collect();
    if text.ends_with('\n') {
        lines.pop();
    }
    lines
        .into_iter()
        .map(|line| {
            if line.is_empty() {
                Vec::new()
            } else {
                vec![Token::new(TokenKind::Normal, line)]
            }
        })
        .collect()
}

/// Token kinds in the order their CSS rules appear: ordered by the compact HTML class, with the
/// unclassed default (`Normal`, whose rule targets `code span` alone) first.
const CSS_ORDER: [TokenKind; 31] = [
    TokenKind::Normal,
    TokenKind::Alert,
    TokenKind::Annotation,
    TokenKind::Attribute,
    TokenKind::BaseN,
    TokenKind::BuiltIn,
    TokenKind::ControlFlow,
    TokenKind::Char,
    TokenKind::Constant,
    TokenKind::Comment,
    TokenKind::CommentVar,
    TokenKind::Documentation,
    TokenKind::DataType,
    TokenKind::DecVal,
    TokenKind::Error,
    TokenKind::Extension,
    TokenKind::Float,
    TokenKind::Function,
    TokenKind::Import,
    TokenKind::Information,
    TokenKind::Keyword,
    TokenKind::Operator,
    TokenKind::Other,
    TokenKind::Preprocessor,
    TokenKind::RegionMarker,
    TokenKind::SpecialChar,
    TokenKind::SpecialString,
    TokenKind::String,
    TokenKind::Variable,
    TokenKind::VerbatimString,
    TokenKind::Warning,
];

/// Token kinds in the order their LaTeX macros are defined: alphabetically by macro name.
const LATEX_ORDER: [TokenKind; 31] = [
    TokenKind::Alert,
    TokenKind::Annotation,
    TokenKind::Attribute,
    TokenKind::BaseN,
    TokenKind::BuiltIn,
    TokenKind::Char,
    TokenKind::Comment,
    TokenKind::CommentVar,
    TokenKind::Constant,
    TokenKind::ControlFlow,
    TokenKind::DataType,
    TokenKind::DecVal,
    TokenKind::Documentation,
    TokenKind::Error,
    TokenKind::Extension,
    TokenKind::Float,
    TokenKind::Function,
    TokenKind::Import,
    TokenKind::Information,
    TokenKind::Keyword,
    TokenKind::Normal,
    TokenKind::Operator,
    TokenKind::Other,
    TokenKind::Preprocessor,
    TokenKind::RegionMarker,
    TokenKind::SpecialChar,
    TokenKind::SpecialString,
    TokenKind::String,
    TokenKind::Variable,
    TokenKind::VerbatimString,
    TokenKind::Warning,
];

/// The fixed head of the CSS: the layout rules shared by every theme, through the line-number
/// pseudo-element's last unconditional declaration. The theme-driven line-number and background
/// declarations, then the per-token color rules, are appended by [`theme_css`].
const CSS_HEAD: &str = "    /* CSS for syntax highlighting */
    html { -webkit-text-size-adjust: 100%; }
    pre > code.sourceCode { white-space: pre; position: relative; }
    pre > code.sourceCode > span { display: inline-block; line-height: 1.25; }
    pre > code.sourceCode > span:empty { height: 1.2em; }
    .sourceCode { overflow: visible; }
    code.sourceCode > span { color: inherit; text-decoration: inherit; }
    div.sourceCode { margin: 1em 0; }
    pre.sourceCode { margin: 0; }
    @media screen {
    div.sourceCode { overflow: auto; }
    }
    @media print {
    pre > code.sourceCode { white-space: pre-wrap; }
    pre > code.sourceCode > span { text-indent: -5em; padding-left: 5em; }
    }
    pre.numberSource code
      { counter-reset: source-line 0; }
    pre.numberSource code > span
      { position: relative; left: -4em; counter-increment: source-line; }
    pre.numberSource code > span > a:first-child::before
      { content: counter(source-line);
        position: relative; left: -1em; text-align: right; vertical-align: baseline;
        border: none; display: inline-block;
        -webkit-touch-callout: none; -webkit-user-select: none;
        -khtml-user-select: none; -moz-user-select: none;
        -ms-user-select: none; user-select: none;
        padding: 0 4px; width: 4em;
";

/// The fixed head of the LaTeX preamble: the verbatim environment the colorized listing runs in,
/// before the theme-driven `Shaded` environment and per-token macros [`theme_latex_macros`] appends.
const LATEX_HEAD: &str = r"\usepackage{fancyvrb}
\newcommand{\VerbBar}{|}
\newcommand{\VERB}{\Verb[commandchars=\\\{\}]}
\DefineVerbatimEnvironment{Highlighting}{Verbatim}{commandchars=\\\{\}}
% Add ',fontsize=\small' for more characters per line
";

/// The `<style>` body a standalone HTML document (or an EPUB page) embeds to color the code blocks a
/// theme describes: the shared layout rules, the theme's line-number and background colors, and one
/// rule per customized token kind. The result carries no trailing newline.
#[must_use]
pub fn theme_css(theme: &Theme) -> String {
    let mut out = String::from(CSS_HEAD);
    if let Some(color) = &theme.line_number_background_color {
        let _ = writeln!(out, "        background-color: {color};");
    }
    if let Some(color) = &theme.line_number_color {
        let _ = writeln!(out, "        color: {color};");
    }
    out.push_str("      }\n");
    match &theme.line_number_color {
        Some(color) => {
            let _ = writeln!(
                out,
                "    pre.numberSource {{ margin-left: 3em; border-left: 1px solid {color};  padding-left: 4px; }}"
            );
        }
        None => out.push_str("    pre.numberSource { margin-left: 3em;  padding-left: 4px; }\n"),
    }
    out.push_str("    div.sourceCode\n");
    // Each of the two frame declarations is present as `prop: value; ` or collapses to a lone space,
    // so a frame with neither color reads `{   }` and one with only a background reads `{  bg; }`.
    let text = theme
        .text_color
        .as_deref()
        .map_or_else(|| " ".to_owned(), |color| format!("color: {color}; "));
    let background = theme.background_color.as_deref().map_or_else(
        || " ".to_owned(),
        |color| format!("background-color: {color}; "),
    );
    let _ = writeln!(out, "      {{ {text}{background}}}");
    out.push_str("    @media screen {\n");
    out.push_str(
        "    pre > code.sourceCode > span > a:first-child::before { text-decoration: underline; }\n",
    );
    out.push_str("    }\n");
    // A kind gets a rule when the theme carries an entry for it — even an entry that sets nothing,
    // which renders as empty braces — but a kind the theme omits entirely gets none.
    for kind in CSS_ORDER {
        let Some(style) = theme.style_for(kind) else {
            continue;
        };
        let mut props = Vec::new();
        if let Some(color) = &style.text_color {
            props.push(format!("color: {color};"));
        }
        if let Some(color) = &style.background_color {
            props.push(format!("background-color: {color};"));
        }
        if style.bold {
            props.push("font-weight: bold;".to_owned());
        }
        if style.italic {
            props.push("font-style: italic;".to_owned());
        }
        if style.underline {
            props.push("text-decoration: underline;".to_owned());
        }
        let inner = props.join(" ");
        let braces = if inner.is_empty() {
            "{ }".to_owned()
        } else {
            format!("{{ {inner} }}")
        };
        let class = kind.html_class();
        if class.is_empty() {
            let _ = writeln!(out, "    code span {braces} /* {} */", kind.style_key());
        } else {
            let _ = writeln!(
                out,
                "    code span.{class} {braces} /* {} */",
                kind.style_key()
            );
        }
    }
    trim_trailing_newline(out)
}

/// The LaTeX preamble a standalone document defines so a colorized listing typesets: the verbatim
/// environment, the shaded frame the theme's background asks for, and one color macro per token
/// kind. The result carries no trailing newline.
#[must_use]
pub fn theme_latex_macros(theme: &Theme) -> String {
    let mut out = String::from(LATEX_HEAD);
    match theme.background_color.as_deref().and_then(hex_channels) {
        Some((r, g, b)) => {
            out.push_str("\\usepackage{framed}\n");
            let _ = writeln!(out, "\\definecolor{{shadecolor}}{{RGB}}{{{r},{g},{b}}}");
            out.push_str("\\newenvironment{Shaded}{\\begin{snugshade}}{\\end{snugshade}}\n");
        }
        None => out.push_str("\\newenvironment{Shaded}{}{}\n"),
    }
    let default_color = theme.text_color.as_deref();
    for kind in LATEX_ORDER {
        let body = latex_token_body(theme.style_for(kind), default_color);
        let _ = writeln!(out, "\\newcommand{{\\{}}}[1]{{{body}}}", kind.latex_macro());
    }
    trim_trailing_newline(out)
}

/// The body of one token's LaTeX macro: `#1` wrapped, innermost to outermost, by its background box,
/// underline, italic, bold, and foreground color — only the attributes the style sets. Unlike the CSS
/// rules, which let a token inherit the block's foreground, every macro carries an explicit color, so a
/// kind that sets none falls back to the theme's default foreground.
fn latex_token_body(style: Option<&TokenStyle>, default_color: Option<&str>) -> String {
    let mut body = String::from("#1");
    let color = style
        .and_then(|s| s.text_color.as_deref())
        .or(default_color);
    if let Some(style) = style {
        if let Some(rgb) = style.background_color.as_deref().and_then(rgb_triplet) {
            body = format!("\\colorbox[rgb]{{{rgb}}}{{{body}}}");
        }
        if style.underline {
            body = format!("\\underline{{{body}}}");
        }
        if style.italic {
            body = format!("\\textit{{{body}}}");
        }
        if style.bold {
            body = format!("\\textbf{{{body}}}");
        }
    }
    if let Some(rgb) = color.and_then(rgb_triplet) {
        body = format!("\\textcolor[rgb]{{{rgb}}}{{{body}}}");
    }
    body
}

/// Parse a `#rrggbb` color into its three 8-bit channels.
fn hex_channels(color: &str) -> Option<(u8, u8, u8)> {
    let hex = color.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let r = u8::from_str_radix(hex.get(0..2)?, 16).ok()?;
    let g = u8::from_str_radix(hex.get(2..4)?, 16).ok()?;
    let b = u8::from_str_radix(hex.get(4..6)?, 16).ok()?;
    Some((r, g, b))
}

/// Render a `#rrggbb` color as the comma-separated `r,g,b` triplet of fractions LaTeX's `rgb` model
/// wants, each to two decimals.
fn rgb_triplet(color: &str) -> Option<String> {
    let (r, g, b) = hex_channels(color)?;
    Some(format!(
        "{:.2},{:.2},{:.2}",
        f64::from(r) / 255.0,
        f64::from(g) / 255.0,
        f64::from(b) / 255.0
    ))
}

/// Drop a single trailing newline, so a generated preamble slots into a template line without
/// leaving a blank line behind it.
fn trim_trailing_newline(mut text: String) -> String {
    if text.ends_with('\n') {
        text.pop();
    }
    text
}
