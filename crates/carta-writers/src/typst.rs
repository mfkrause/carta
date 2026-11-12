//! Render Typst markup, using code-mode functions for constructs without markup syntax.

use carta_ast::{
    Attr, Block, Caption, Document, Inline, ListAttributes, ListNumberDelim, ListNumberStyle,
};
use carta_core::{Extension, Result, TocStyle, WrapMode, Writer, WriterOptions};

use crate::common::{FILL_COLUMN, list_is_tight};

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
    fn render_document(&self, document: &Document, options: &WriterOptions) -> Result<String> {
        #[cfg(feature = "highlight")]
        let mut unhighlighted_document;
        #[cfg(feature = "highlight")]
        let document = if options.highlight.disable_native {
            unhighlighted_document = document.clone();
            carta_core::walk::for_each_document_block(&mut unhighlighted_document, &mut |block| {
                if let Block::CodeBlock(attributes, _) = block {
                    attributes.classes.clear();
                }
            });
            &unhighlighted_document
        } else {
            document
        };
        let width = options.columns.unwrap_or(FILL_COLUMN);
        let smart = options.extensions.contains(Extension::Smart);
        let body = blocks(&document.blocks, width, options.wrap, smart);
        Ok(body.trim_matches('\n').to_owned())
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
}

/// How a rendered block leaves its last line: still open, ended, or followed by a blank line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BlockEnd {
    Open,
    Line,
    Blank,
}

pub(super) fn block_end(block: &Block) -> BlockEnd {
    match block {
        Block::Plain(_) | Block::RawBlock(..) | Block::Div(..) => BlockEnd::Open,
        Block::Header(..) => BlockEnd::Line,
        _ => BlockEnd::Blank,
    }
}

/// Render a block sequence without the spacing its last block asks for.
fn blocks(items: &[Block], width: usize, wrap: WrapMode, smart: bool) -> String {
    let mut out = spaced_blocks(items, width, wrap, smart);
    out.truncate(out.trim_end_matches('\n').len());
    out
}

/// Render a block sequence with its boundary spacing: a leading newline when a blank line must
/// precede the first block, and a trailing newline or blank line as the last block ends.
pub(super) fn spaced_blocks(items: &[Block], width: usize, wrap: WrapMode, smart: bool) -> String {
    let mut out = String::new();
    let mut end = BlockEnd::Open;
    let mut has_text = false;
    for item in items {
        let piece = block(item, width, wrap, smart);
        if piece.is_empty() {
            if matches!(item, Block::Para(_)) {
                end = BlockEnd::Blank;
            }
            continue;
        }
        if matches!(item, Block::HorizontalRule) {
            end = BlockEnd::Blank;
        }
        if has_text {
            out.push('\n');
        }
        if end == BlockEnd::Blank {
            out.push('\n');
        }
        out.push_str(&piece);
        has_text = true;
        end = block_end(item);
    }
    match end {
        BlockEnd::Blank if has_text => out.push_str("\n\n"),
        BlockEnd::Blank | BlockEnd::Line => out.push('\n'),
        BlockEnd::Open => {}
    }
    out
}

/// Block content set on the lines below an opening delimiter, each indented; nothing when empty.
fn nested_lines(body: &str, indent: &str) -> String {
    if body.is_empty() {
        String::new()
    } else {
        indent_continuation(&format!("\n{body}"), indent)
    }
}

/// Spaced block content in brackets opened mid-line, with later lines indented. A leading blank
/// line first ends the bracket's line, and a closing bracket on a line of its own is indented too.
pub(super) fn bracketed(body: &str, indent: &str) -> String {
    let mut out = String::from("[");
    if body.starts_with('\n') {
        out.push('\n');
    }
    out.push_str(&indent_continuation(body, indent));
    if body.ends_with('\n') {
        out.push_str(indent);
    }
    out.push(']');
    out
}

/// Prefix every line after the first with `indent`, leaving blank lines bare.
pub(super) fn indent_continuation(body: &str, indent: &str) -> String {
    let mut out = String::new();
    for (index, line) in body.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
            if !line.is_empty() {
                out.push_str(indent);
            }
        }
        out.push_str(line);
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
                "#quote(block: true)[{}\n]",
                nested_lines(&blocks(items, width, wrap, smart), "")
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
        Block::Figure(attr, caption, items) => figure(attr, caption, items, width, wrap, smart),
        Block::Div(attr, items) => div(attr, items, width, wrap, smart),
    }
}

fn header(
    level: i64,
    attr: &Attr,
    items: &[Inline],
    width: usize,
    wrap: WrapMode,
    smart: bool,
) -> String {
    let text = inline_run(items, width, wrap, smart);
    let heading = if attr.classes.iter().any(|class| class == "unnumbered") {
        format!("#heading(level: {level}, numbering: none)[{text}]")
    } else if level > 32 {
        format!("#heading(depth: {level})[{text}]")
    } else {
        let depth = usize::try_from(level.max(1)).unwrap_or(1);
        format!("{} {text}", "=".repeat(depth))
    };
    match label(&attr.id) {
        Some(rendered) => format!("{heading}\n{rendered}"),
        None => heading,
    }
}

/// A trailing label for a node carrying an identifier.
pub(super) fn label(id: &str) -> Option<String> {
    if id.is_empty() {
        return None;
    }
    let target = label_target(id);
    if target.starts_with('<') {
        Some(target)
    } else {
        Some(format!("#{target}"))
    }
}

fn label_target(id: &str) -> String {
    if id.is_empty() {
        String::new()
    } else if id
        .chars()
        .all(|ch| ch.is_alphanumeric() || "_-.:".contains(ch))
    {
        format!("<{id}>")
    } else {
        format!("label(\"{}\")", escape_string(id))
    }
}

fn code_block(attr: &Attr, text: &str) -> String {
    let fence = backtick_fence(text);
    let trailing = if text.is_empty() || text.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    match attr.classes.first() {
        Some(language) => format!("{fence}{language}\n{text}{trailing}{fence}"),
        None => format!("{fence}\n{text}{trailing}{fence}"),
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
    let mut body = blocks(item, width.saturating_sub(marker.len()), wrap, smart);
    if body.is_empty() {
        return marker.trim_end().to_owned();
    }
    // A leading blank line first ends the marker's line.
    if body.starts_with('\n') {
        body.insert(0, '\n');
    }
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
        let body = definitions
            .iter()
            .map(|definition| blocks(definition, width, wrap, smart))
            .filter(|body| !body.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        lines.push(format!(
            "/ {}: #block[{}\n]",
            escape_term_colons(&inline_run(term, width, wrap, smart)),
            nested_lines(&body, "")
        ));
    }
    lines.join("\n\n")
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
    fill_inlines(&lines.join(&Inline::LineBreak), width, wrap, smart)
}

fn div(attr: &Attr, items: &[Block], width: usize, wrap: WrapMode, smart: bool) -> String {
    let mut block = format!("#block[\n{}", spaced_blocks(items, width, wrap, smart));
    if !block.ends_with('\n') {
        block.push('\n');
    }
    block.push(']');
    match label(&attr.id) {
        Some(rendered) => format!("{block} {rendered}"),
        None => block,
    }
}

fn figure(
    attr: &Attr,
    caption: &Caption,
    items: &[Block],
    width: usize,
    wrap: WrapMode,
    smart: bool,
) -> String {
    let inner = match figure_image(items) {
        Some(image) => image,
        None => bracketed(
            &spaced_blocks(items, width.saturating_sub(2), wrap, smart),
            "  ",
        ),
    };
    let figure = format!(
        "#figure({inner},\n  caption: [{}\n  ]\n)",
        nested_lines(
            &blocks(&caption.long, width.saturating_sub(4), wrap, smart),
            "    "
        )
    );
    match label(&attr.id) {
        Some(label) => format!("{figure}\n{label}"),
        None => figure,
    }
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
