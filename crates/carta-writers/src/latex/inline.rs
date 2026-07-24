//! Inline rendering: styled spans, links, images, math, and footnotes.

use carta_ast::{Attr, Block, Inline, MathType, QuoteType, Target, to_plain_text};
use carta_core::WrapMode;

use crate::common::{Piece, attribute_value, indent_block, label_matches_url};

use super::code::{
    code_block_fallback, highlighted_code_block, highlighted_code_inline, idiomatic_code_inline,
};
use super::escaping::{
    EscapeMode, cross_reference_label, escape, escape_smart, escape_url, is_latex_format,
};
use super::{Dialect, Hl, block_to_string, header_anchor, phantom_label};

/// Render an inline list. After a quote span, a thin space separates its closing delimiter from a
/// following quotation mark so the two marks do not run together into one glyph.
#[allow(clippy::too_many_arguments)]
pub(super) fn push_inlines(
    inlines: &[Inline],
    out: &mut Vec<Piece>,
    width: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
    in_header: bool,
    in_soul: bool,
    hl: Hl<'_>,
) {
    let mut remaining = inlines.iter().peekable();
    while let Some(inline) = remaining.next() {
        push_inline(
            inline, out, width, dialect, wrap, smart, in_header, in_soul, hl,
        );
        if matches!(inline, Inline::Quoted(..))
            && let Some(Inline::Str(text)) = remaining.peek()
            && text.chars().next().is_some_and(is_quotation_mark)
        {
            out.push(Piece::text("\\,"));
        }
    }
}

/// Whether a character is a quotation mark that would visually merge with a preceding quote span's
/// closing delimiter. The grave accent is not a quotation mark and is excluded.
fn is_quotation_mark(ch: char) -> bool {
    matches!(ch, '\u{2018}' | '\u{2019}' | '\u{201C}' | '\u{201D}' | '\'')
}

#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
fn push_inline(
    inline: &Inline,
    out: &mut Vec<Piece>,
    width: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
    in_header: bool,
    in_soul: bool,
    hl: Hl<'_>,
) {
    match inline {
        Inline::Str(text) => out.push(Piece::text(escape_smart(text, EscapeMode::Text, smart))),
        Inline::Emph(inlines) => {
            wrap_command(
                "\\emph{", inlines, out, width, dialect, wrap, smart, in_header, in_soul, hl,
            );
        }
        Inline::Strong(inlines) => {
            wrap_command(
                "\\textbf{",
                inlines,
                out,
                width,
                dialect,
                wrap,
                smart,
                in_header,
                in_soul,
                hl,
            );
        }
        Inline::Underline(inlines) => {
            wrap_command(
                "\\ul{", inlines, out, width, dialect, wrap, smart, in_header, true, hl,
            );
        }
        Inline::Strikeout(inlines) => {
            wrap_command(
                "\\st{", inlines, out, width, dialect, wrap, smart, in_header, true, hl,
            );
        }
        Inline::Superscript(inlines) => {
            wrap_command(
                "\\textsuperscript{",
                inlines,
                out,
                width,
                dialect,
                wrap,
                smart,
                in_header,
                in_soul,
                hl,
            );
        }
        Inline::Subscript(inlines) => {
            wrap_command(
                "\\textsubscript{",
                inlines,
                out,
                width,
                dialect,
                wrap,
                smart,
                in_header,
                in_soul,
                hl,
            );
        }
        Inline::SmallCaps(inlines) => {
            wrap_command(
                "\\textsc{",
                inlines,
                out,
                width,
                dialect,
                wrap,
                smart,
                in_header,
                in_soul,
                hl,
            );
        }
        Inline::Quoted(kind, inlines) => {
            let (open, close) = match kind {
                QuoteType::SingleQuote => ('\u{2018}', '\u{2019}'),
                QuoteType::DoubleQuote => ('\u{201C}', '\u{201D}'),
            };
            out.push(Piece::text(escape_smart(
                &open.to_string(),
                EscapeMode::Text,
                smart,
            )));
            push_inlines(
                inlines, out, width, dialect, wrap, smart, in_header, in_soul, hl,
            );
            out.push(Piece::text(escape_smart(
                &close.to_string(),
                EscapeMode::Text,
                smart,
            )));
        }
        Inline::Cite(_, inlines) => {
            push_inlines(
                inlines, out, width, dialect, wrap, smart, in_header, in_soul, hl,
            );
        }
        Inline::Code(attr, text) => {
            let rendered = highlighted_code_inline(attr, text, hl)
                .or_else(|| idiomatic_code_inline(attr, text, hl))
                .unwrap_or_else(|| format!("\\texttt{{{}}}", escape(text, EscapeMode::Code)));
            out.push(Piece::text(rendered));
        }
        Inline::Space => out.push(Piece::Space),
        Inline::SoftBreak => out.push(Piece::Soft),
        Inline::LineBreak => {
            out.push(Piece::text("\\\\"));
            out.push(Piece::Hard);
        }
        Inline::Math(kind, text) => {
            // `\(…\)`/`\[…\]` are fragile inside character-splitting `soul` commands; dollars survive.
            let rendered = match (kind, in_soul) {
                (MathType::InlineMath, false) => format!("\\({text}\\)"),
                (MathType::DisplayMath, false) => format!("\\[{text}\\]"),
                (MathType::InlineMath, true) => format!("${text}$"),
                (MathType::DisplayMath, true) => format!("$${text}$$"),
            };
            out.push(Piece::text(rendered));
        }
        Inline::RawInline(format, text) => {
            if is_latex_format(&format.0) {
                out.push(Piece::text(text.to_string()));
            }
        }
        Inline::Link(attr, inlines, target) => {
            push_link(
                attr, inlines, target, out, width, dialect, wrap, smart, in_header, in_soul, hl,
            );
        }
        Inline::Image(attr, inlines, target) => {
            out.push(Piece::text(image(attr, inlines, target, smart)));
        }
        Inline::Span(attr, inlines) => {
            let mut open = if attr.id.is_empty() {
                String::new()
            } else if in_header {
                header_anchor(&attr.id)
            } else {
                phantom_label(&attr.id)
            };
            open.push('{');
            out.push(Piece::text(open));
            push_inlines(
                inlines, out, width, dialect, wrap, smart, in_header, in_soul, hl,
            );
            out.push(Piece::text("}"));
        }
        Inline::Note(blocks) => {
            out.push(Piece::text(note(blocks, width, dialect, wrap, smart, hl)));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn wrap_command(
    open: &str,
    inlines: &[Inline],
    out: &mut Vec<Piece>,
    width: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
    in_header: bool,
    in_soul: bool,
    hl: Hl<'_>,
) {
    out.push(Piece::text(open.to_owned()));
    push_inlines(
        inlines, out, width, dialect, wrap, smart, in_header, in_soul, hl,
    );
    out.push(Piece::text("}"));
}

#[allow(clippy::too_many_arguments)]
fn push_link(
    attr: &Attr,
    inlines: &[Inline],
    target: &Target,
    out: &mut Vec<Piece>,
    width: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
    in_header: bool,
    in_soul: bool,
    hl: Hl<'_>,
) {
    if !attr.id.is_empty() {
        out.push(Piece::text(if in_header {
            header_anchor(&attr.id)
        } else {
            phantom_label(&attr.id)
        }));
    }
    // A `#` fragment is a cross-reference resolved through `\hyperref`, not `\href`, and is not
    // boxed inside underline/strikeout; only external links are.
    if let Some(reference) = target.url.strip_prefix('#') {
        out.push(Piece::text(format!(
            "\\hyperref[{}]{{",
            cross_reference_label(reference)
        )));
        for inline in inlines {
            push_inline(
                inline, out, width, dialect, wrap, smart, in_header, in_soul, hl,
            );
        }
        out.push(Piece::text("}"));
        return;
    }
    // `soul` splits its argument into characters, breaking a hyperlink's box; `\mbox` keeps the link whole.
    let (box_open, box_close) = if in_soul { ("\\mbox{", "}") } else { ("", "") };
    let url = escape_url(&target.url);
    if let [Inline::Str(text)] = inlines
        && label_matches_url(text, &target.url)
    {
        out.push(Piece::text(format!("{box_open}\\url{{{url}}}{box_close}")));
        return;
    }
    // A mailto link whose text is the bare address renders the address verbatim, unstyled.
    if let [Inline::Str(text)] = inlines
        && let Some(address) = target.url.strip_prefix("mailto:")
        && text == address
    {
        let address = escape_url(address);
        out.push(Piece::text(format!(
            "{box_open}\\href{{{url}}}{{\\nolinkurl{{{address}}}}}{box_close}"
        )));
        return;
    }
    out.push(Piece::text(format!("{box_open}\\href{{{url}}}{{")));
    push_inlines(
        inlines, out, width, dialect, wrap, smart, in_header, in_soul, hl,
    );
    out.push(Piece::text(format!("}}{box_close}")));
}

fn image(attr: &Attr, inlines: &[Inline], target: &Target, smart: bool) -> String {
    let svg = is_svg(&target.url);
    // The SVG include command has no alt key; other images emit it whenever a description is present.
    let alt_option = if svg || inlines.is_empty() {
        String::new()
    } else {
        format!(
            ",alt={{{}}}",
            escape_smart(&to_plain_text(inlines), EscapeMode::Text, smart)
        )
    };
    let command = if svg { "includesvg" } else { "includegraphics" };
    let url = escape_url(&target.url);

    let width = attribute_value(attr, "width").and_then(Dimension::parse);
    let height = attribute_value(attr, "height").and_then(Dimension::parse);
    if width.is_none() && height.is_none() {
        return format!("\\pandocbounded{{\\{command}[keepaspectratio{alt_option}]{{{url}}}}}");
    }

    let width_option = match &width {
        Some(dimension) => dimension.render("\\linewidth"),
        None => "\\linewidth".to_owned(),
    };
    let height_option = match &height {
        Some(dimension) => dimension.render("\\textheight"),
        None => "\\textheight".to_owned(),
    };
    let aspect = if width.is_some() && height.is_some() {
        ""
    } else {
        ",keepaspectratio"
    };
    format!("\\{command}[width={width_option},height={height_option}{aspect}{alt_option}]{{{url}}}")
}

/// Whether an image URL names an SVG file, i.e. its path's final extension is `svg`. The extension
/// is the text after the last `.` in the last `/`-delimited segment, so a trailing query string
/// (which is part of the extension under this rule) means the URL is not treated as an SVG.
fn is_svg(url: &str) -> bool {
    let segment = url.rsplit('/').next().unwrap_or(url);
    matches!(segment.rsplit_once('.'), Some((_, ext)) if ext.eq_ignore_ascii_case("svg"))
}

/// A parsed image dimension. A pixel or bare number is expressed in inches at 96 pixels per inch; a
/// percentage is expressed as a fraction of a reference length; any other recognized unit is kept
/// verbatim.
pub(super) enum Dimension {
    Length(String),
    Percent(f64),
}

impl Dimension {
    pub(super) fn parse(value: &str) -> Option<Dimension> {
        let value = value.trim();
        let split = value
            .find(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
            .unwrap_or(value.len());
        let (number, unit) = value.split_at(split);
        let number: f64 = number.parse().ok()?;
        match unit.to_ascii_lowercase().as_str() {
            "" | "px" => Some(Dimension::Length(format!(
                "{}in",
                trim_number(number / 96.0)
            ))),
            "%" => Some(Dimension::Percent(number)),
            "in" | "cm" | "mm" | "pt" | "pc" | "em" => {
                Some(Dimension::Length(format!("{}{unit}", trim_number(number))))
            }
            _ => None,
        }
    }

    pub(super) fn render(&self, reference: &str) -> String {
        match self {
            Dimension::Length(rendered) => rendered.clone(),
            Dimension::Percent(percent) => format!("{}{reference}", trim_number(percent / 100.0)),
        }
    }
}

/// Format a number to at most five fractional digits, dropping trailing zeros.
pub(super) fn trim_number(value: f64) -> String {
    let formatted = format!("{value:.5}");
    formatted
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_owned()
}

/// Render a footnote as an inline `\footnote{…}`. Its blocks hang two columns under the opening so
/// continuation paragraphs align with the first; a code block instead sits flush against the margin
/// in a `Verbatim` environment, since verbatim content cannot be indented, and pushes the closing
/// brace onto its own line.
fn note(
    blocks: &[Block],
    base_width: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
    hl: Hl<'_>,
) -> String {
    let width = base_width.saturating_sub(2);
    let mut parts: Vec<String> = Vec::new();
    let mut ends_with_code = false;
    for block in blocks {
        let (rendered, is_code) = match block {
            Block::CodeBlock(attr, text) => (
                highlighted_code_block(attr, text, hl)
                    .unwrap_or_else(|| code_block_fallback(attr, text, hl, "Verbatim")),
                true,
            ),
            _ => (
                block_to_string(block, width, 0, dialect, wrap, smart, hl),
                false,
            ),
        };
        if rendered.is_empty() {
            continue;
        }
        ends_with_code = is_code;
        let indented = if is_code {
            rendered
        } else if parts.is_empty() {
            indent_block(&rendered, "", "  ")
        } else {
            indent_block(&rendered, "  ", "  ")
        };
        parts.push(indented);
    }
    let body = parts.join("\n\n");
    let closing = if ends_with_code { "\n}" } else { "}" };
    let opening = match dialect {
        Dialect::Article => "\\footnote{",
        Dialect::Slide { .. } => "\\footnote<\\value{beamerpauses}->[frame]{",
    };
    format!("{opening}{body}{closing}")
}
