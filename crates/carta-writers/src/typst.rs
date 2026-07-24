//! Typst writer: renders the document model to Typst markup.
//!
//! Block structure is conveyed through Typst's line-oriented markup; paragraph text is wrapped to a
//! fill column. Constructs that have no native markup form are emitted as code-mode function calls
//! (`#strong[..]`, `#link("..")[..]`, `#figure(..)`, `#table(..)`, …). Markup-significant characters
//! in literal text are backslash-escaped, some only where they could open a block or line marker.
//! Output carries no trailing newline; the caller appends one. The targeted syntax is described in
//! `vendor/typst/spec.md`.

use carta_ast::{
    Attr, Block, Caption, Document, Inline, ListAttributes, ListNumberDelim, ListNumberStyle,
};
use carta_core::{Extension, Result, TocStyle, WrapMode, Writer, WriterOptions};

use crate::common::FILL_COLUMN;

mod escape;
mod inline;
mod tables;

use escape::{escape_string, image_call, raw_passthrough};
use inline::{fill_inlines, inline_run};
use tables::render_table;

/// Renders a document to Typst markup (no trailing newline).
#[derive(Debug, Default, Clone, Copy)]
pub struct TypstWriter;

impl Writer for TypstWriter {
    fn write(&self, document: &Document, options: &WriterOptions) -> Result<String> {
        let width = options.columns.unwrap_or(FILL_COLUMN);
        let smart = options.extensions.contains(Extension::Smart);
        let body = blocks(&document.blocks, width, options.wrap, smart);
        Ok(body.trim_end_matches('\n').to_owned())
    }

    fn default_template(&self) -> Option<&'static str> {
        Some(include_str!("templates/default.typst"))
    }

    fn body_ends_with_newline(&self) -> bool {
        true
    }

    fn toc_style(&self) -> TocStyle {
        TocStyle::Native
    }

    fn numbers_sections_natively(&self) -> bool {
        true
    }
}

/// A fragment of rendered inline content awaiting line filling. Text fragments are breakable on the
/// spaces between them and may carry a marker character at their head that must be escaped should the
/// fragment open a physical line; atomic fragments (markup function calls) never break or escape.
#[derive(Debug, Clone)]
enum Fragment {
    /// A run of escaped literal text with no interior break point.
    Text(String),
    /// An atomic markup token (`#strong[..]`, a link, …) carried whole.
    Atom(String),
    /// A breakable space.
    Space,
    /// A soft line break from the source: a breakable reflow point under `Auto`, a space under
    /// `None`, and a kept physical line break under `Preserve`.
    Soft,
    /// A forced line break, rendered as ` \ ` and not breaking the physical line.
    LineBreak,
}

/// Render a top-level (or nested) block sequence. Every block is separated from the next by a blank
/// line, except that a header is followed by a single newline.
fn blocks(items: &[Block], width: usize, wrap: WrapMode, smart: bool) -> String {
    let mut out = String::new();
    let mut previous_is_header = false;
    for item in items {
        let piece = block(item, width, wrap, smart);
        if piece.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
            if !previous_is_header {
                out.push('\n');
            }
        }
        out.push_str(&piece);
        previous_is_header = matches!(item, Block::Header(..));
    }
    out
}

fn block(value: &Block, width: usize, wrap: WrapMode, smart: bool) -> String {
    match value {
        Block::Plain(items) | Block::Para(items) => fill_inlines(items, width, wrap, smart),
        Block::Header(level, attr, items) => header(*level, attr, items, width, wrap, smart),
        Block::CodeBlock(attr, text) => code_block(attr, text),
        Block::RawBlock(format, text) => raw_passthrough(format, text),
        Block::BlockQuote(items) => {
            format!(
                "#quote(block: true)[\n{}\n]",
                blocks(items, width, wrap, smart)
            )
        }
        Block::BulletList(items) => bullet_list(items, width, wrap, smart),
        Block::OrderedList(list_attrs, items) => {
            ordered_list(list_attrs, items, width, wrap, smart)
        }
        Block::DefinitionList(items) => definition_list(items, width, wrap, smart),
        Block::HorizontalRule => "#horizontalrule".to_owned(),
        Block::LineBlock(lines) => line_block(lines, width, wrap, smart),
        Block::Table(table) => render_table(table, width, wrap, smart),
        Block::Figure(_, caption, items) => figure(caption, items, width, wrap, smart),
        Block::Div(attr, items) => div(attr, items, width, wrap, smart),
    }
}

fn header(
    level: i32,
    attr: &Attr,
    items: &[Inline],
    width: usize,
    wrap: WrapMode,
    smart: bool,
) -> String {
    let text = inline_run(items, width, wrap, smart);
    let heading = if attr.classes.iter().any(|class| class == "unnumbered") {
        format!("#heading(level: {level}, numbering: none)[{text}]")
    } else {
        // The cap keeps an absurd level from forcing an unbounded marker allocation.
        const MAX_HEADING_LEVEL: i32 = 32;
        let depth = usize::try_from(level.clamp(1, MAX_HEADING_LEVEL)).unwrap_or(1);
        format!("{} {text}", "=".repeat(depth))
    };
    match label(&attr.id) {
        Some(rendered) => format!("{heading}\n{rendered}"),
        None => heading,
    }
}

/// A trailing label for a node carrying an id. Typst's `<name>` short form holds an identifier-like
/// id; an id containing whitespace is emitted through `#label("..")`, with interior whitespace
/// collapsed to single spaces.
pub(super) fn label(id: &str) -> Option<String> {
    if id.is_empty() {
        return None;
    }
    if id.contains(char::is_whitespace) {
        let normalized: String = id.split_whitespace().collect::<Vec<_>>().join(" ");
        Some(format!("#label(\"{}\")", escape_string(&normalized)))
    } else {
        Some(format!("<{id}>"))
    }
}

fn code_block(attr: &Attr, text: &str) -> String {
    let fence = backtick_fence(text);
    match attr.classes.first() {
        Some(language) => format!("{fence}{language}\n{text}\n{fence}"),
        None => format!("{fence}\n{text}\n{fence}"),
    }
}

/// A backtick fence at least one tick longer than the longest backtick run in the payload, so the
/// fence cannot be closed early by the content.
fn backtick_fence(text: &str) -> String {
    let mut longest = 0usize;
    let mut current = 0usize;
    for ch in text.chars() {
        if ch == '`' {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    "`".repeat(longest.max(2) + 1)
}

fn bullet_list(items: &[Vec<Block>], width: usize, wrap: WrapMode, smart: bool) -> String {
    let loose = !list_is_tight(items);
    let mut lines = Vec::new();
    for item in items {
        lines.push(list_item("- ", item, width, wrap, smart));
    }
    lines.join(if loose { "\n\n" } else { "\n" })
}

fn ordered_list(
    attrs: &ListAttributes,
    items: &[Vec<Block>],
    width: usize,
    wrap: WrapMode,
    smart: bool,
) -> String {
    let loose = !list_is_tight(items);
    let mut lines = Vec::new();
    for item in items {
        lines.push(list_item("+ ", item, width, wrap, smart));
    }
    let body = lines.join(if loose { "\n\n" } else { "\n" });
    if is_default_enum(attrs) {
        body
    } else {
        format!(
            "#block[\n#set enum(numbering: \"{}\", start: {})\n{body}\n]",
            enum_numbering(attrs),
            attrs.start,
        )
    }
}

/// Whether every item is empty or opens with a [`Block::Plain`]; such a list renders tight.
fn list_is_tight(items: &[Vec<Block>]) -> bool {
    items
        .iter()
        .all(|item| matches!(item.first(), None | Some(Block::Plain(_))))
}

/// Whether an ordered list uses Typst's implicit `+` numbering: decimal style, period delimiter, and
/// a start of one. Anything else is rendered through an explicit `#set enum` rule.
fn is_default_enum(attrs: &ListAttributes) -> bool {
    attrs.start == 1
        && matches!(
            attrs.style,
            ListNumberStyle::DefaultStyle | ListNumberStyle::Decimal
        )
        && matches!(
            attrs.delim,
            ListNumberDelim::DefaultDelim | ListNumberDelim::Period
        )
}

/// The `numbering` pattern string for an explicit enumeration: the style's sample numeral wrapped in
/// the delimiter (`1.`, `a)`, `(I)`, …).
fn enum_numbering(attrs: &ListAttributes) -> String {
    let numeral = match attrs.style {
        ListNumberStyle::LowerAlpha => "a",
        ListNumberStyle::UpperAlpha => "A",
        ListNumberStyle::LowerRoman => "i",
        ListNumberStyle::UpperRoman => "I",
        ListNumberStyle::DefaultStyle | ListNumberStyle::Decimal | ListNumberStyle::Example => "1",
    };
    match attrs.delim {
        ListNumberDelim::OneParen => format!("{numeral})"),
        ListNumberDelim::TwoParens => format!("({numeral})"),
        ListNumberDelim::Period | ListNumberDelim::DefaultDelim => format!("{numeral}."),
    }
}

/// Render one list item: the marker on its first line, with every continuation line indented to
/// align under the marker's text column.
fn list_item(marker: &str, item: &[Block], width: usize, wrap: WrapMode, smart: bool) -> String {
    let body = blocks(item, width, wrap, smart);
    let indent = " ".repeat(marker.len());
    let mut out = String::new();
    for (index, line) in body.lines().enumerate() {
        if index > 0 {
            out.push('\n');
            if !line.is_empty() {
                out.push_str(&indent);
            }
        }
        out.push_str(line);
    }
    format!("{marker}{out}")
}

fn definition_list(
    items: &[(Vec<Inline>, Vec<Vec<Block>>)],
    width: usize,
    wrap: WrapMode,
    smart: bool,
) -> String {
    let mut lines = Vec::new();
    for (term, definitions) in items {
        let body = blocks(
            &definitions.iter().flatten().cloned().collect::<Vec<_>>(),
            width,
            wrap,
            smart,
        );
        lines.push(format!(
            "/ {}: #block[\n{body}\n]",
            escape_term_colons(&inline_run(term, width, wrap, smart))
        ));
    }
    lines.join("\n")
}

/// In a definition term, a colon would close the term, so escape every colon in the rendered
/// markup, except those inside a string-literal argument such as a link URL.
fn escape_term_colons(term: &str) -> String {
    let mut out = String::with_capacity(term.len());
    let mut in_string = false;
    let mut escaped = false;
    for ch in term.chars() {
        if !escaped && ch == '"' {
            in_string = !in_string;
        }
        if ch == ':' && !in_string && !escaped {
            out.push('\\');
        }
        escaped = ch == '\\' && !escaped;
        out.push(ch);
    }
    out
}

fn line_block(lines: &[Vec<Inline>], width: usize, wrap: WrapMode, smart: bool) -> String {
    let rendered: Vec<String> = lines
        .iter()
        .map(|line| inline_run(line, width, wrap, smart))
        .collect();
    rendered.join(" \\ ")
}

fn div(attr: &Attr, items: &[Block], width: usize, wrap: WrapMode, smart: bool) -> String {
    let body = blocks(items, width, wrap, smart);
    let trailing = match items.last() {
        Some(Block::Plain(_)) => "",
        _ => "\n",
    };
    let block = format!("#block[\n{body}{trailing}\n]");
    match label(&attr.id) {
        Some(rendered) => format!("{block} {rendered}"),
        None => block,
    }
}

fn figure(caption: &Caption, items: &[Block], width: usize, wrap: WrapMode, smart: bool) -> String {
    let inner = match figure_image(items) {
        Some(image) => image,
        None => format!("[{}]", blocks(items, width, wrap, smart)),
    };
    format!(
        "#figure({inner},\n  caption: [\n    {}\n  ]\n)",
        blocks(&caption.long, width, wrap, smart).trim_end_matches('\n')
    )
}

/// When a figure's body is a single image, the bare `image(..)` call carried directly as the
/// figure's content.
fn figure_image(items: &[Block]) -> Option<String> {
    match items {
        [Block::Plain(inlines) | Block::Para(inlines)] => match inlines.as_slice() {
            [Inline::Image(attr, alt, target)] => Some(image_call(attr, alt, target)),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests;
