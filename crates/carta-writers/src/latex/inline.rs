//! Inline rendering: styled spans, links, images, math, and footnotes.

use carta_ast::{Attr, Block, Inline, MathType, QuoteType, Target, to_plain_text};
use carta_core::WrapMode;

use crate::common::{
    Indent, IndentScope, Piece, attribute_value, fill_indented, indent_block, label_matches_url,
    scope_indents,
};

use super::code::{
    code_block_fallback, highlighted_code_block, highlighted_code_inline, idiomatic_code_inline,
};
use super::escaping::{
    EscapeMode, cross_reference_label, escape, escape_smart, escape_url, is_latex_format,
};
use super::{Dialect, Hl, block_to_string, header_anchor, phantom_label};

/// Inline pieces and their continuation indentation scopes.
#[derive(Default, Clone)]
pub(super) struct InlineLayout {
    pieces: Vec<Piece>,
    scopes: Vec<IndentScope>,
    indent: usize,
}

impl InlineLayout {
    pub(super) fn push(&mut self, piece: Piece) {
        self.pieces.push(piece);
    }

    pub(super) fn append(&mut self, mut other: Self) {
        let offset = self.pieces.len();
        for scope in &mut other.scopes {
            scope.start += offset;
            scope.end += offset;
        }
        self.pieces.extend(other.pieces);
        self.scopes.extend(other.scopes);
    }

    pub(super) fn render(&self, width: usize, wrap: WrapMode) -> String {
        fill_indented(&self.pieces, &self.scopes, width, wrap)
    }

    pub(super) fn flatten(&self) -> String {
        let indents = scope_indents(&self.scopes, self.pieces.len());
        let mut out = String::new();
        for (index, piece) in self.pieces.iter().enumerate() {
            match piece {
                Piece::Text(text) => {
                    if out.ends_with('\n') && !text.starts_with('\n') && !text.is_empty() {
                        out.push_str(&" ".repeat(indents.get(index).copied().unwrap_or(0)));
                    }
                    out.push_str(text);
                }
                Piece::Space | Piece::Soft => out.push(' '),
                Piece::Hard => out.push('\n'),
            }
        }
        out
    }

    fn with_indent<T>(&mut self, indent: Indent, work: impl FnOnce(&mut Self) -> T) -> T {
        let previous = self.indent;
        self.indent = indent.apply(previous);
        let index = self.scopes.len();
        self.scopes.push(IndentScope {
            start: self.pieces.len(),
            end: self.pieces.len(),
            indent,
        });
        let result = work(self);
        if let Some(scope) = self.scopes.get_mut(index) {
            scope.end = self.pieces.len();
        }
        self.indent = previous;
        result
    }
}

/// Render an inline list. After a quote span, a thin space separates its closing delimiter from a
/// following quotation mark so the two marks do not run together into one glyph.
#[allow(clippy::too_many_arguments)]
pub(super) fn push_inlines(
    inlines: &[Inline],
    out: &mut InlineLayout,
    width: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
    in_header: bool,
    in_soul: bool,
    hl: Hl<'_>,
) {
    carta_core::with_stack(|| {
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
    });
}

/// Whether a character is a quotation mark that would visually merge with a preceding quote span's
/// closing delimiter. The grave accent is not a quotation mark and is excluded.
fn is_quotation_mark(ch: char) -> bool {
    matches!(ch, '\u{2018}' | '\u{2019}' | '\u{201C}' | '\u{201D}' | '\'')
}

#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
fn push_inline(
    inline: &Inline,
    out: &mut InlineLayout,
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
            note(blocks, out, width, dialect, wrap, smart, hl);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn wrap_command(
    open: &str,
    inlines: &[Inline],
    out: &mut InlineLayout,
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
    out: &mut InlineLayout,
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
    // Internal references do not need the external-link boxes used inside soul commands.
    if let Some(reference) = target.url.strip_prefix('#') {
        let label = cross_reference_label(reference);
        out.push(Piece::text(if dialect.is_slide() {
            format!("\\hyperlink{{{label}}}{{")
        } else {
            format!("\\hyperref[{label}]{{")
        }));
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

enum NoteContent<'a> {
    Inlines(&'a [Inline]),
    Literal(String),
    Code(String),
}

/// Render a footnote with scoped continuation indentation and literal code at the margin.
fn note(
    blocks: &[Block],
    out: &mut InlineLayout,
    base_width: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
    hl: Hl<'_>,
) {
    let opening = match dialect {
        Dialect::Article => "\\footnote{",
        Dialect::Slide { .. } => "\\footnote<\\value{beamerpauses}->[frame]{",
    };
    out.push(Piece::text(opening));
    let width = base_width.saturating_sub(2);
    let mut ends_with_code = false;
    out.with_indent(Indent::Add(2), |out| {
        let mut first = true;
        for block in blocks {
            let content = match block {
                Block::Plain(inlines) | Block::Para(inlines) => NoteContent::Inlines(inlines),
                Block::CodeBlock(attr, text) => NoteContent::Code(
                    highlighted_code_block(attr, text, hl)
                        .unwrap_or_else(|| code_block_fallback(attr, text, hl, "Verbatim")),
                ),
                _ => NoteContent::Literal(block_to_string(block, width, 0, dialect, wrap, smart, hl)),
            };
            if matches!(&content, NoteContent::Literal(text) | NoteContent::Code(text) if text.is_empty()) {
                continue;
            }
            let previous_break = if !first && matches!(out.pieces.last(), Some(Piece::Soft | Piece::Hard)) {
                out.pieces.pop()
            } else {
                None
            };
            let block_start = out.pieces.len();
            let is_code = matches!(content, NoteContent::Code(_));
            let append = |out: &mut InlineLayout| {
                if !first {
                    out.push(Piece::text(format!("\n\n{}", " ".repeat(out.indent))));
                }
                let content_start = out.pieces.len();
                match content {
                    NoteContent::Code(text) => out.push(Piece::text(text)),
                    NoteContent::Literal(text) => out.push(Piece::text(indent_block(&text, "", &" ".repeat(out.indent)))),
                    NoteContent::Inlines(inlines) => {
                        push_inlines(inlines, out, width, dialect, wrap, smart, false, false, hl);
                        let mut soft_break = false;
                        while matches!(out.pieces.last(), Some(Piece::Space | Piece::Soft)) {
                            soft_break |= matches!(out.pieces.pop(), Some(Piece::Soft)) && wrap == WrapMode::Preserve;
                        }
                        if soft_break && !matches!(out.pieces.last(), Some(Piece::Hard)) {
                            out.push(Piece::Soft);
                        }
                    }
                }
                out.pieces.len() > content_start
            };
            let has_content = if is_code {
                out.with_indent(Indent::Reset, append)
            } else {
                append(out)
            };
            if has_content {
                first = false;
                ends_with_code = is_code;
            } else {
                out.pieces.truncate(block_start);
                if let Some(previous) = previous_break {
                    out.push(previous);
                }
            }
        }
    });
    if ends_with_code {
        out.push(Piece::text(format!("\n{}", " ".repeat(out.indent))));
    }
    out.push(Piece::text("}"));
}
