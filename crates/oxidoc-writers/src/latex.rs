//! LaTeX writer: renders the document model to a LaTeX document fragment.
//!
//! Output is a body fragment (no preamble or `\begin{document}`) wrapped at a fill column of 72;
//! the wrap counts the literal LaTeX, markup included. Document metadata is not emitted. Syntax
//! highlighting is neutralized: a code block renders as a `verbatim` environment and inline code as
//! `\texttt{…}`, regardless of any language class. The result carries no trailing newline; the
//! caller appends one. This format has no public specification.

use oxidoc_ast::{
    Attr, Block, Caption, Document, Inline, ListAttributes, ListNumberDelim, ListNumberStyle,
    MathType, QuoteType, Target, to_plain_text,
};
use oxidoc_core::{Result, Writer, WriterOptions};

use crate::common::{FILL_COLUMN, Piece, fill, indent_block, list_is_tight, wrap_delim};

/// Renders a document to a LaTeX fragment.
#[derive(Debug, Default, Clone, Copy)]
pub struct LatexWriter;

impl Writer for LatexWriter {
    fn write(&self, document: &Document, _options: &WriterOptions) -> Result<String> {
        let body = render_blocks(&document.blocks, FILL_COLUMN, 0);
        Ok(body.trim_end_matches('\n').to_owned())
    }
}

/// Selects the escaping policy for a run of literal text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EscapeMode {
    /// Running prose.
    Text,
    /// Inside a `\texttt{…}` group, where spaces and a few extra glyphs gain escapes.
    Code,
}

/// Render a block sequence with a blank line between blocks, dropping those that produce no output.
fn render_blocks(blocks: &[Block], width: usize, enum_depth: usize) -> String {
    blocks
        .iter()
        .map(|block| block_to_string(block, width, enum_depth))
        .filter(|rendered| !rendered.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn block_to_string(block: &Block, width: usize, enum_depth: usize) -> String {
    match block {
        Block::Plain(inlines) | Block::Para(inlines) => inlines_to_string(inlines, width),
        Block::Header(level, attr, inlines) => header(*level, attr, inlines, width),
        Block::CodeBlock(attr, text) => code_block(attr, text),
        Block::RawBlock(format, text) => {
            if is_latex_format(&format.0) {
                text.strip_suffix('\n').unwrap_or(text).to_owned()
            } else {
                String::new()
            }
        }
        Block::BlockQuote(blocks) => format!(
            "\\begin{{quote}}\n{}\n\\end{{quote}}",
            render_blocks(blocks, width, enum_depth)
        ),
        Block::BulletList(items) => bullet_list(items, width, enum_depth),
        Block::OrderedList(attrs, items) => ordered_list(attrs, items, width, enum_depth),
        Block::DefinitionList(items) => definition_list(items, width, enum_depth),
        Block::HorizontalRule => {
            "\\begin{center}\\rule{0.5\\linewidth}{0.5pt}\\end{center}".to_owned()
        }
        Block::LineBlock(lines) => line_block(lines, width),
        Block::Div(attr, blocks) => {
            let body = render_blocks(blocks, width, enum_depth);
            if attr.id.is_empty() {
                body
            } else {
                format!("{}\n{body}", phantom_label(&attr.id))
            }
        }
        Block::Figure(attr, caption, blocks) => figure(attr, caption, blocks, width, enum_depth),
        Block::Table(_) => todo!("latex writer: render tables"),
    }
}

fn header(level: i32, attr: &Attr, inlines: &[Inline], width: usize) -> String {
    let command = match level {
        1 => "section",
        2 => "subsection",
        3 => "subsubsection",
        4 => "paragraph",
        5 => "subparagraph",
        _ => return inlines_to_string(inlines, width),
    };
    let unnumbered = attr.classes.iter().any(|class| class == "unnumbered");
    let star = if unnumbered { "*" } else { "" };
    let inner = inline_pieces(inlines);

    let mut content = vec![Piece::Text(format!("\\{command}{star}{{"))];
    if needs_texorpdfstring(inlines) {
        content.push(Piece::Text("\\texorpdfstring{".to_owned()));
        content.extend(inner.iter().cloned());
        let pdf = escape(&to_plain_text(inlines), EscapeMode::Text);
        content.push(Piece::Text(format!("}}{{{pdf}}}")));
    } else {
        content.extend(inner.iter().cloned());
    }
    content.push(Piece::Text("}".to_owned()));
    if !attr.id.is_empty() {
        content.push(Piece::Text(format!("\\label{{{}}}", attr.id)));
    }
    let heading = fill(&content, width);

    if unnumbered {
        let mut toc = vec![Piece::Text(format!(
            "\\addcontentsline{{toc}}{{{command}}}{{"
        ))];
        toc.extend(inner);
        toc.push(Piece::Text("}".to_owned()));
        format!("{heading}\n{}", fill(&toc, width))
    } else {
        heading
    }
}

fn code_block(attr: &Attr, text: &str) -> String {
    let body = text.strip_suffix('\n').unwrap_or(text);
    let verbatim = format!("\\begin{{verbatim}}\n{body}\n\\end{{verbatim}}");
    if attr.id.is_empty() {
        verbatim
    } else {
        format!("{}%\n{verbatim}", phantom_label(&attr.id))
    }
}

/// The anchor markup emitted for an element carrying an identifier.
fn phantom_label(id: &str) -> String {
    format!("\\protect\\phantomsection\\label{{{id}}}")
}

fn bullet_list(items: &[Vec<Block>], width: usize, enum_depth: usize) -> String {
    let mut lines = vec!["\\begin{itemize}".to_owned()];
    if list_is_tight(items) {
        lines.push("\\tightlist".to_owned());
    }
    for item in items {
        lines.push(list_item(item, width, enum_depth));
    }
    lines.push("\\end{itemize}".to_owned());
    lines.join("\n")
}

fn ordered_list(
    attrs: &ListAttributes,
    items: &[Vec<Block>],
    width: usize,
    enum_depth: usize,
) -> String {
    let depth = enum_depth + 1;
    let counter = enum_counter(depth);
    let mut lines = vec!["\\begin{enumerate}".to_owned()];
    if let Some(label) = label_definition(attrs, counter) {
        lines.push(label);
    }
    if attrs.start != 1 {
        lines.push(format!(
            "\\setcounter{{{counter}}}{{{}}}",
            attrs.start.saturating_sub(1)
        ));
    }
    if list_is_tight(items) {
        lines.push("\\tightlist".to_owned());
    }
    for item in items {
        lines.push(list_item(item, width, depth));
    }
    lines.push("\\end{enumerate}".to_owned());
    lines.join("\n")
}

/// Render one list item: its blocks indented two columns under an `\item` line.
fn list_item(item: &[Block], width: usize, enum_depth: usize) -> String {
    let body = render_blocks(item, width.saturating_sub(2), enum_depth);
    let content = indent_block(&body, "  ", "  ");
    if content.is_empty() {
        "\\item".to_owned()
    } else {
        format!("\\item\n{content}")
    }
}

fn definition_list(
    items: &[(Vec<Inline>, Vec<Vec<Block>>)],
    width: usize,
    enum_depth: usize,
) -> String {
    let mut lines = vec!["\\begin{description}".to_owned()];
    if is_tight_definitions(items) {
        lines.push("\\tightlist".to_owned());
    }
    for (term, definitions) in items {
        let header = format!("\\item[{}]", inlines_to_string(term, width));
        let bodies: Vec<String> = definitions
            .iter()
            .map(|definition| render_blocks(definition, width, enum_depth))
            .filter(|rendered| !rendered.is_empty())
            .collect();
        if bodies.is_empty() {
            lines.push(header);
        } else {
            lines.push(format!("{header}\n{}", bodies.join("\n\n")));
        }
    }
    lines.push("\\end{description}".to_owned());
    lines.join("\n")
}

fn line_block(lines: &[Vec<Inline>], width: usize) -> String {
    lines
        .iter()
        .map(|line| inlines_to_string(line, width))
        .collect::<Vec<_>>()
        .join("\\\\\n")
}

fn figure(
    attr: &Attr,
    caption: &Caption,
    blocks: &[Block],
    width: usize,
    enum_depth: usize,
) -> String {
    let mut parts = vec![
        "\\begin{figure}".to_owned(),
        "\\centering".to_owned(),
        render_blocks(blocks, width, enum_depth),
    ];
    if !attr.id.is_empty() {
        parts.push(format!("\\label{{{}}}", attr.id));
    }
    let caption_inlines = caption_text(caption);
    if !caption_inlines.is_empty() {
        parts.push(format!(
            "\\caption{{{}}}",
            inlines_to_string(&caption_inlines, width)
        ));
    }
    parts.push("\\end{figure}".to_owned());
    parts.join("\n")
}

/// Collect a caption's inline content from its block-level body.
fn caption_text(caption: &Caption) -> Vec<Inline> {
    let mut out = Vec::new();
    for block in &caption.long {
        if let Block::Plain(inlines) | Block::Para(inlines) = block {
            out.extend(inlines.iter().cloned());
        }
    }
    out
}

/// The leading `\def\labelenum…` an ordered list carries, or `None` when both numeral style and
/// delimiter are the renderer defaults (where the built-in label suffices).
fn label_definition(attrs: &ListAttributes, counter: &str) -> Option<String> {
    if matches!(attrs.style, ListNumberStyle::DefaultStyle)
        && matches!(attrs.delim, ListNumberDelim::DefaultDelim)
    {
        return None;
    }
    let numeral = numeral_command(&attrs.style, counter);
    let label = wrap_delim(&numeral, &attrs.delim);
    Some(format!("\\def\\label{counter}{{{label}}}"))
}

fn numeral_command(style: &ListNumberStyle, counter: &str) -> String {
    let command = match style {
        ListNumberStyle::DefaultStyle | ListNumberStyle::Decimal | ListNumberStyle::Example => {
            "arabic"
        }
        ListNumberStyle::LowerAlpha => "alph",
        ListNumberStyle::UpperAlpha => "Alph",
        ListNumberStyle::LowerRoman => "roman",
        ListNumberStyle::UpperRoman => "Roman",
    };
    format!("\\{command}{{{counter}}}")
}

/// The LaTeX enumerate counter name for a nesting depth (`enumi`, `enumii`, …), capped at the four
/// levels LaTeX provides.
fn enum_counter(depth: usize) -> &'static str {
    match depth {
        0 | 1 => "enumi",
        2 => "enumii",
        3 => "enumiii",
        _ => "enumiv",
    }
}

fn is_tight_definitions(items: &[(Vec<Inline>, Vec<Vec<Block>>)]) -> bool {
    items
        .iter()
        .all(|(_, definitions)| list_is_tight(definitions))
}

/// Whether a heading needs a `\texorpdfstring` wrapper: it carries an inline that produces no plain
/// PDF-bookmark text on its own (anything beyond literal text and spaces).
fn needs_texorpdfstring(inlines: &[Inline]) -> bool {
    inlines
        .iter()
        .any(|inline| !matches!(inline, Inline::Str(_) | Inline::Space | Inline::SoftBreak))
}

fn inlines_to_string(inlines: &[Inline], width: usize) -> String {
    fill(&inline_pieces(inlines), width)
}

fn inline_pieces(inlines: &[Inline]) -> Vec<Piece> {
    let mut out = Vec::new();
    for inline in inlines {
        push_inline(inline, &mut out);
    }
    out
}

fn push_inline(inline: &Inline, out: &mut Vec<Piece>) {
    match inline {
        Inline::Str(text) => out.push(Piece::Text(escape(text, EscapeMode::Text))),
        Inline::Emph(inlines) => wrap_command("\\emph{", inlines, out),
        Inline::Strong(inlines) => wrap_command("\\textbf{", inlines, out),
        Inline::Underline(inlines) => wrap_command("\\ul{", inlines, out),
        Inline::Strikeout(inlines) => wrap_command("\\st{", inlines, out),
        Inline::Superscript(inlines) => wrap_command("\\textsuperscript{", inlines, out),
        Inline::Subscript(inlines) => wrap_command("\\textsubscript{", inlines, out),
        Inline::SmallCaps(inlines) => wrap_command("\\textsc{", inlines, out),
        Inline::Quoted(kind, inlines) => {
            let (open, close) = quote_marks(kind);
            out.push(Piece::Text(open.to_owned()));
            for inline in inlines {
                push_inline(inline, out);
            }
            out.push(Piece::Text(close.to_owned()));
        }
        Inline::Cite(_, inlines) => {
            for inline in inlines {
                push_inline(inline, out);
            }
        }
        Inline::Code(_, text) => {
            out.push(Piece::Text(format!(
                "\\texttt{{{}}}",
                escape(text, EscapeMode::Code)
            )));
        }
        Inline::Space | Inline::SoftBreak => out.push(Piece::Space),
        Inline::LineBreak => {
            out.push(Piece::Text("\\\\".to_owned()));
            out.push(Piece::Hard);
        }
        Inline::Math(kind, text) => {
            let rendered = match kind {
                MathType::InlineMath => format!("\\({text}\\)"),
                MathType::DisplayMath => format!("\\[{text}\\]"),
            };
            out.push(Piece::Text(rendered));
        }
        Inline::RawInline(format, text) => {
            if is_latex_format(&format.0) {
                out.push(Piece::Text(text.clone()));
            }
        }
        Inline::Link(attr, inlines, target) => push_link(attr, inlines, target, out),
        Inline::Image(attr, inlines, target) => out.push(Piece::Text(image(attr, inlines, target))),
        Inline::Span(attr, inlines) => {
            let mut open = if attr.id.is_empty() {
                String::new()
            } else {
                phantom_label(&attr.id)
            };
            open.push('{');
            out.push(Piece::Text(open));
            for inline in inlines {
                push_inline(inline, out);
            }
            out.push(Piece::Text("}".to_owned()));
        }
        Inline::Note(blocks) => out.push(Piece::Text(note(blocks))),
    }
}

fn wrap_command(open: &str, inlines: &[Inline], out: &mut Vec<Piece>) {
    out.push(Piece::Text(open.to_owned()));
    for inline in inlines {
        push_inline(inline, out);
    }
    out.push(Piece::Text("}".to_owned()));
}

fn push_link(attr: &Attr, inlines: &[Inline], target: &Target, out: &mut Vec<Piece>) {
    if !attr.id.is_empty() {
        out.push(Piece::Text(phantom_label(&attr.id)));
    }
    let url = escape_url(&target.url);
    if let [Inline::Str(text)] = inlines
        && *text == target.url
    {
        out.push(Piece::Text(format!("\\url{{{url}}}")));
        return;
    }
    out.push(Piece::Text(format!("\\href{{{url}}}{{")));
    for inline in inlines {
        push_inline(inline, out);
    }
    out.push(Piece::Text("}".to_owned()));
}

fn image(attr: &Attr, inlines: &[Inline], target: &Target) -> String {
    let alt = to_plain_text(inlines);
    let alt_option = if alt.is_empty() {
        String::new()
    } else {
        format!(",alt={{{}}}", escape(&alt, EscapeMode::Text))
    };
    let url = escape_url(&target.url);

    let width = attr_value(attr, "width").and_then(Dimension::parse);
    let height = attr_value(attr, "height").and_then(Dimension::parse);
    if width.is_none() && height.is_none() {
        return format!(
            "\\pandocbounded{{\\includegraphics[keepaspectratio{alt_option}]{{{url}}}}}"
        );
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
    format!(
        "\\includegraphics[width={width_option},height={height_option}{aspect}{alt_option}]{{{url}}}"
    )
}

/// A parsed image dimension. A pixel or bare number is expressed in inches at 96 pixels per inch; a
/// percentage is expressed as a fraction of a reference length; any other recognized unit is kept
/// verbatim.
enum Dimension {
    Length(String),
    Percent(f64),
}

impl Dimension {
    fn parse(value: &str) -> Option<Dimension> {
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

    fn render(&self, reference: &str) -> String {
        match self {
            Dimension::Length(rendered) => rendered.clone(),
            Dimension::Percent(percent) => format!("{}{reference}", trim_number(percent / 100.0)),
        }
    }
}

/// Format a number to at most five fractional digits, dropping trailing zeros.
fn trim_number(value: f64) -> String {
    let formatted = format!("{value:.5}");
    formatted
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_owned()
}

fn attr_value<'a>(attr: &'a Attr, key: &str) -> Option<&'a str> {
    attr.attributes
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

/// Render a footnote as an inline `\footnote{…}`; its blocks hang two columns under the opening so
/// continuation paragraphs align with the first.
fn note(blocks: &[Block]) -> String {
    let body = render_blocks(blocks, FILL_COLUMN.saturating_sub(2), 0);
    format!("{}}}", indent_block(&body, "\\footnote{", "  "))
}

fn quote_marks(kind: &QuoteType) -> (&'static str, &'static str) {
    match kind {
        QuoteType::SingleQuote => ("`", "'"),
        QuoteType::DoubleQuote => ("``", "''"),
    }
}

fn is_latex_format(format: &str) -> bool {
    format.eq_ignore_ascii_case("latex") || format.eq_ignore_ascii_case("tex")
}

/// Escape a run of literal text for the given context.
fn escape(text: &str, mode: EscapeMode) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        let next = chars.peek().copied();
        match ch {
            '&' | '%' | '#' | '_' | '$' | '{' | '}' => {
                out.push('\\');
                out.push(ch);
            }
            '^' => out.push_str("\\^{}"),
            '[' => out.push_str("{[}"),
            ']' => out.push_str("{]}"),
            '~' => push_control_word(&mut out, "\\textasciitilde", next, mode),
            '\\' => push_control_word(&mut out, "\\textbackslash", next, mode),
            '<' => push_control_word(&mut out, "\\textless", next, mode),
            '>' => push_control_word(&mut out, "\\textgreater", next, mode),
            '|' => push_control_word(&mut out, "\\textbar", next, mode),
            '\'' => push_control_word(&mut out, "\\textquotesingle", next, mode),
            '-' if next == Some('-') => out.push_str("-\\/"),
            ' ' if mode == EscapeMode::Code => out.push_str("\\ "),
            '`' if mode == EscapeMode::Code => out.push_str("\\textasciigrave{}"),
            '\u{a0}' if mode == EscapeMode::Text => out.push('~'),
            '\u{2026}' if mode == EscapeMode::Text => {
                push_control_word(&mut out, "\\ldots", next, mode);
            }
            '\u{2013}' if mode == EscapeMode::Text => out.push_str("--"),
            '\u{2014}' if mode == EscapeMode::Text => out.push_str("---"),
            '\u{2018}' if mode == EscapeMode::Text => out.push('`'),
            '\u{2019}' if mode == EscapeMode::Text => out.push('\''),
            '\u{201C}' if mode == EscapeMode::Text => out.push_str("``"),
            '\u{201D}' if mode == EscapeMode::Text => out.push_str("''"),
            other => out.push(other),
        }
    }
    out
}

/// Emit a control-word command and the separator that stops it from absorbing the following
/// character. In code context the command always closes with an empty group; in text context the
/// separator depends on what follows: a space before a letter, an empty group before whitespace or
/// the end of the run, and nothing before other glyphs (which already terminate the command).
fn push_control_word(out: &mut String, command: &str, next: Option<char>, mode: EscapeMode) {
    out.push_str(command);
    match mode {
        EscapeMode::Code => out.push_str("{}"),
        EscapeMode::Text => match next {
            Some(following) if following.is_alphabetic() => out.push(' '),
            Some(following) if following.is_whitespace() => out.push_str("{}"),
            None => out.push_str("{}"),
            Some(_) => {}
        },
    }
}

/// Escape a URL for `\href`/`\url`/`\includegraphics`: percent-encode the bytes LaTeX cannot carry
/// in a URL argument, map a backslash to a forward slash, and escape the surviving `#` and `%`.
fn escape_url(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    for ch in url.chars() {
        match ch {
            '\\' => out.push('/'),
            '#' => out.push_str("\\#"),
            '%' => out.push_str("\\%"),
            ' ' | '"' | '<' | '>' | '[' | ']' | '^' | '`' | '{' | '|' | '}' => {
                percent_encode(ch, &mut out);
            }
            other if !other.is_ascii() || (other as u32) < 0x20 => percent_encode(other, &mut out),
            other => out.push(other),
        }
    }
    out
}

fn percent_encode(ch: char, out: &mut String) {
    let mut buffer = [0u8; 4];
    for byte in ch.encode_utf8(&mut buffer).bytes() {
        out.push_str("\\%");
        out.push(hex_digit(byte >> 4));
        out.push(hex_digit(byte & 0x0f));
    }
}

fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        _ => (b'A' + value - 10) as char,
    }
}
