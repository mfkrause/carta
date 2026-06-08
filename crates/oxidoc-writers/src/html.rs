//! HTML writer: renders the document model to an html5 fragment.
//!
//! Syntax highlighting and TeX math rendering are neutralized: code blocks render as a plain
//! `<pre><code>` and math as a MathJax-style `\(…\)` / `\[…\]` passthrough span. Those two
//! subsystems are deferred (see `docs/plans/slice-1-commonmark-html.md`). Output is a fragment with
//! no trailing newline; the caller appends one.

use std::fmt::Write as _;

use oxidoc_ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Document, Inline, ListAttributes,
    ListNumberStyle, MathType, Row, Table, TableBody, Target, Text, to_plain_text,
};
use oxidoc_core::{Result, Writer, WriterOptions};

use crate::common::{FILL_COLUMN, is_known_attribute, is_wide, quote_marks};

/// Renders a document to an html5 fragment.
#[derive(Debug, Default, Clone, Copy)]
pub struct HtmlWriter;

impl Writer for HtmlWriter {
    fn write(&self, document: &Document, _options: &WriterOptions) -> Result<String> {
        let mut state = State::default();
        let mut out = String::new();
        state.blocks(&mut out, &document.blocks);
        state.push_footnote_section(&mut out);
        let filled = restore(&reflow(&out));
        Ok(filled.trim_end_matches('\n').to_owned())
    }
}

/// Sentinel marking a breakable inline space while the document is assembled as a flat string.
/// [`reflow`] later turns each into either a single space or a line break to fill to
/// [`FILL_COLUMN`]. A literal `U+0000` from document content is preserved
/// verbatim, so content can legitimately contain this scalar; [`protect_char`] encodes any such
/// occurrence before reflow and [`restore`] decodes it afterwards, keeping the channel unambiguous.
const BREAK: char = '\u{0}';

/// Escape introducer that protects a literal [`BREAK`] (or a literal introducer) appearing in
/// document content from being mistaken for a writer-inserted break during [`reflow`]. `U+0001` is
/// a control scalar the writer never emits structurally; [`protect_char`] encodes and [`restore`]
/// reverses it.
const ESCAPE: char = '\u{1}';

/// Tag following an [`ESCAPE`] introducer that stands for one content `U+0000`. The pair is removed
/// again by [`restore`]; any printable char distinct from [`ESCAPE`] would serve.
const BREAK_TAG: char = '0';

/// Where an attribute set is being rendered, which selects the field order. Most elements emit
/// `id`, then `class`, then key/value pairs; headers emit `class`, then key/value pairs, then `id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttrOrder {
    Standard,
    Header,
}

/// Carries the footnote bodies accumulated while rendering, so notes can be collected inline and
/// emitted as a section at the end of the document.
#[derive(Debug, Default)]
struct State {
    footnotes: Vec<String>,
}

impl State {
    /// Render a block sequence into `out`, one block per line.
    fn blocks(&mut self, out: &mut String, blocks: &[Block]) {
        for (index, block) in blocks.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            self.block(out, block);
        }
    }

    fn block(&mut self, out: &mut String, block: &Block) {
        match block {
            Block::Plain(inlines) => self.inlines(out, inlines),
            Block::Para(inlines) => {
                out.push_str("<p>");
                self.inlines(out, inlines);
                out.push_str("</p>");
            }
            Block::Header(level, attr, inlines) => {
                let tag = header_tag(*level);
                let _ = write!(out, "<{tag}{}>", render_attr(attr, AttrOrder::Header));
                self.inlines(out, inlines);
                let _ = write!(out, "</{tag}>");
            }
            Block::CodeBlock(attr, text) => {
                let _ = write!(
                    out,
                    "<pre{}><code>{}</code></pre>",
                    render_attr(attr, AttrOrder::Standard),
                    escape_attr(text)
                );
            }
            Block::RawBlock(format, text) => out.push_str(&raw_passthrough(&format.0, text)),
            Block::BlockQuote(blocks) => {
                out.push_str("<blockquote>\n");
                self.blocks(out, blocks);
                out.push_str("\n</blockquote>");
            }
            Block::BulletList(items) => self.bullet_list(out, items),
            Block::OrderedList(attrs, items) => self.ordered_list(out, attrs, items),
            Block::DefinitionList(items) => self.definition_list(out, items),
            Block::Div(attr, blocks) => {
                let _ = writeln!(out, "<div{}>", render_attr(attr, AttrOrder::Standard));
                self.blocks(out, blocks);
                out.push_str("\n</div>");
            }
            Block::Figure(attr, caption, blocks) => self.figure(out, attr, caption, blocks),
            Block::HorizontalRule => out.push_str("<hr />"),
            Block::LineBlock(lines) => self.line_block(out, lines),
            Block::Table(table) => self.table(out, table),
        }
    }

    fn bullet_list(&mut self, out: &mut String, items: &[Vec<Block>]) {
        out.push_str("<ul>\n");
        self.list_items(out, items);
        out.push_str("\n</ul>");
    }

    fn ordered_list(&mut self, out: &mut String, attrs: &ListAttributes, items: &[Vec<Block>]) {
        out.push_str("<ol");
        if attrs.start != 1 {
            let _ = write!(out, " start=\"{}\"", attrs.start);
        }
        if matches!(attrs.style, ListNumberStyle::Example) {
            out.push_str(" class=\"example\"");
        }
        if let Some(kind) = ordered_list_type(&attrs.style) {
            let _ = write!(out, " type=\"{kind}\"");
        }
        out.push_str(">\n");
        self.list_items(out, items);
        out.push_str("\n</ol>");
    }

    /// Render each list item's blocks (newline-joined, no surrounding padding) wrapped in `<li>`.
    fn list_items(&mut self, out: &mut String, items: &[Vec<Block>]) {
        for (index, item) in items.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            out.push_str("<li>");
            self.blocks(out, item);
            out.push_str("</li>");
        }
    }

    fn definition_list(&mut self, out: &mut String, items: &[(Vec<Inline>, Vec<Vec<Block>>)]) {
        out.push_str("<dl>");
        for (term, definitions) in items {
            out.push_str("\n<dt>");
            self.inlines(out, term);
            out.push_str("</dt>");
            for definition in definitions {
                out.push_str("\n<dd>\n");
                self.blocks(out, definition);
                out.push_str("\n</dd>");
            }
        }
        out.push_str("\n</dl>");
    }

    fn figure(&mut self, out: &mut String, attr: &Attr, caption: &Caption, blocks: &[Block]) {
        let _ = writeln!(out, "<figure{}>", render_attr(attr, AttrOrder::Standard));
        self.blocks(out, blocks);
        if !caption.long.is_empty() {
            let hidden = if is_implicit_figure(caption, blocks) {
                " aria-hidden=\"true\""
            } else {
                ""
            };
            let _ = write!(out, "\n<figcaption{hidden}>");
            self.blocks(out, &caption.long);
            out.push_str("</figcaption>");
        }
        out.push_str("\n</figure>");
    }

    fn line_block(&mut self, out: &mut String, lines: &[Vec<Inline>]) {
        out.push_str("<div class=\"line-block\">");
        for (index, line) in lines.iter().enumerate() {
            if index > 0 {
                out.push_str("<br />\n");
            }
            self.inlines(out, line);
        }
        out.push_str("</div>");
    }

    fn table(&mut self, out: &mut String, table: &Table) {
        let _ = write!(
            out,
            "<table{}{}>",
            render_attr(&table.attr, AttrOrder::Standard),
            table_width_style(&table.col_specs)
        );
        if !table.caption.long.is_empty() {
            out.push_str("\n<caption>");
            self.blocks(out, &table.caption.long);
            out.push_str("</caption>");
        }
        let aligns: Vec<Alignment> = table
            .col_specs
            .iter()
            .map(|spec| spec.align.clone())
            .collect();
        out.push_str(&colgroup(&table.col_specs));
        if !table.head.rows.is_empty() {
            out.push_str("\n<thead>\n");
            self.rows(out, &table.head.rows, &aligns, true);
            out.push_str("\n</thead>");
        }
        for body in &table.bodies {
            self.table_body(out, body, &aligns);
        }
        if !table.foot.rows.is_empty() {
            out.push_str("\n<tfoot>\n");
            self.rows(out, &table.foot.rows, &aligns, false);
            out.push_str("\n</tfoot>");
        }
        out.push_str("\n</table>");
    }

    fn table_body(&mut self, out: &mut String, body: &TableBody, aligns: &[Alignment]) {
        out.push_str("\n<tbody>\n");
        let mut first = true;
        for row in &body.head {
            if !first {
                out.push('\n');
            }
            first = false;
            self.row(out, row, aligns, true);
        }
        for row in &body.body {
            if !first {
                out.push('\n');
            }
            first = false;
            self.row(out, row, aligns, false);
        }
        out.push_str("\n</tbody>");
    }

    fn rows(&mut self, out: &mut String, rows: &[Row], aligns: &[Alignment], header: bool) {
        for (index, row) in rows.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            self.row(out, row, aligns, header);
        }
    }

    fn row(&mut self, out: &mut String, row: &Row, aligns: &[Alignment], header: bool) {
        out.push_str("<tr>\n");
        for (index, cell) in row.cells.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            self.cell(out, cell, aligns.get(index), header);
        }
        out.push_str("\n</tr>");
    }

    fn cell(&mut self, out: &mut String, cell: &Cell, col_align: Option<&Alignment>, header: bool) {
        let tag = if header { "th" } else { "td" };
        let effective = match &cell.align {
            Alignment::AlignDefault => col_align.unwrap_or(&Alignment::AlignDefault),
            explicit => explicit,
        };
        let _ = write!(out, "<{tag}");
        if let Some(style) = alignment_style(effective) {
            let _ = write!(out, "{BREAK}style=\"{style}\"");
        }
        if cell.col_span != 1 {
            let _ = write!(out, "{BREAK}colspan=\"{}\"", cell.col_span);
        }
        if cell.row_span != 1 {
            let _ = write!(out, "{BREAK}rowspan=\"{}\"", cell.row_span);
        }
        out.push('>');
        self.blocks(out, &cell.content);
        let _ = write!(out, "</{tag}>");
    }

    fn inlines(&mut self, out: &mut String, inlines: &[Inline]) {
        for inline in inlines {
            self.inline(out, inline);
        }
    }

    fn inline(&mut self, out: &mut String, inline: &Inline) {
        match inline {
            Inline::Str(text) => out.push_str(&escape_text(text)),
            Inline::Emph(inlines) => self.wrap(out, "em", inlines),
            Inline::Strong(inlines) => self.wrap(out, "strong", inlines),
            Inline::Strikeout(inlines) => self.wrap(out, "del", inlines),
            Inline::Superscript(inlines) => self.wrap(out, "sup", inlines),
            Inline::Subscript(inlines) => self.wrap(out, "sub", inlines),
            Inline::Underline(inlines) => self.wrap(out, "u", inlines),
            Inline::SmallCaps(inlines) => {
                out.push_str("<span class=\"smallcaps\">");
                self.inlines(out, inlines);
                out.push_str("</span>");
            }
            Inline::Quoted(kind, inlines) => {
                let (open, close) = quote_marks(kind);
                out.push(open);
                self.inlines(out, inlines);
                out.push(close);
            }
            Inline::Code(attr, text) => {
                let _ = write!(
                    out,
                    "<code{}>{}</code>",
                    render_attr(attr, AttrOrder::Standard),
                    escape_text(text)
                );
            }
            Inline::Space | Inline::SoftBreak => out.push(BREAK),
            Inline::LineBreak => out.push_str("<br />\n"),
            Inline::Math(kind, text) => {
                let (class, open, close) = match kind {
                    MathType::InlineMath => ("inline", "\\(", "\\)"),
                    MathType::DisplayMath => ("display", "\\[", "\\]"),
                };
                let _ = write!(
                    out,
                    "<span class=\"math {class}\">{open}{}{close}</span>",
                    escape_text(text)
                );
            }
            Inline::RawInline(format, text) => out.push_str(&raw_passthrough(&format.0, text)),
            Inline::Link(attr, inlines, target) => self.link(out, attr, inlines, target),
            Inline::Image(attr, inlines, target) => out.push_str(&image(attr, inlines, target)),
            Inline::Span(attr, inlines) => {
                let _ = write!(out, "<span{}>", render_attr(attr, AttrOrder::Standard));
                self.inlines(out, inlines);
                out.push_str("</span>");
            }
            Inline::Cite(citations, inlines) => {
                let ids: Vec<&str> = citations
                    .iter()
                    .map(|citation| citation.id.as_str())
                    .collect();
                let _ = write!(
                    out,
                    "<span class=\"citation\" data-cites=\"{}\">",
                    escape_attr(&ids.join(" "))
                );
                self.inlines(out, inlines);
                out.push_str("</span>");
            }
            Inline::Note(blocks) => self.note(out, blocks),
        }
    }

    fn wrap(&mut self, out: &mut String, tag: &str, inlines: &[Inline]) {
        let _ = write!(out, "<{tag}>");
        self.inlines(out, inlines);
        let _ = write!(out, "</{tag}>");
    }

    fn link(&mut self, out: &mut String, attr: &Attr, inlines: &[Inline], target: &Target) {
        let _ = write!(
            out,
            "<a{BREAK}href=\"{}\"{}{}>",
            escape_attr(&target.url),
            render_attr(attr, AttrOrder::Standard),
            title_attr(&target.title)
        );
        self.inlines(out, inlines);
        out.push_str("</a>");
    }

    fn note(&mut self, out: &mut String, blocks: &[Block]) {
        let number = self.footnotes.len() + 1;
        let backlink = format!(
            "<a{BREAK}href=\"#fnref{number}\"{BREAK}class=\"footnote-back\"{BREAK}role=\"doc-backlink\">\u{21a9}\u{fe0e}</a>"
        );
        let body = self.note_body(blocks, &backlink);
        self.footnotes
            .push(format!("<li{BREAK}id=\"fn{number}\">{body}</li>"));
        let _ = write!(
            out,
            "<a{BREAK}href=\"#fn{number}\"{BREAK}class=\"footnote-ref\"{BREAK}id=\"fnref{number}\"{BREAK}role=\"doc-noteref\"><sup>{number}</sup></a>"
        );
    }

    /// Render a footnote's blocks, appending the backlink inside the final paragraph when the last
    /// block is one, else as a bare trailing element (an unwrapped `Plain`) of its own. The body is
    /// returned as its own value because notes are gathered for a trailing section.
    fn note_body(&mut self, blocks: &[Block], backlink: &str) -> String {
        let mut body = String::new();
        if let Some((Block::Para(inlines), rest)) = blocks.split_last() {
            self.blocks(&mut body, rest);
            append_trailing_newline(&mut body);
            body.push_str("<p>");
            self.inlines(&mut body, inlines);
            body.push_str(backlink);
            body.push_str("</p>");
        } else {
            self.blocks(&mut body, blocks);
            append_trailing_newline(&mut body);
            body.push_str(backlink);
        }
        body
    }

    fn push_footnote_section(&self, out: &mut String) {
        if self.footnotes.is_empty() {
            return;
        }
        let _ = write!(
            out,
            "\n<section{BREAK}id=\"footnotes\"{BREAK}class=\"footnotes footnotes-end-of-document\"{BREAK}role=\"doc-endnotes\">\n<hr />\n<ol>\n"
        );
        for (index, note) in self.footnotes.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            out.push_str(note);
        }
        out.push_str("\n</ol>\n</section>");
    }
}

fn image(attr: &Attr, inlines: &[Inline], target: &Target) -> String {
    let alt = to_plain_text(inlines);
    let alt_attr = if alt.is_empty() {
        String::new()
    } else {
        format!("{BREAK}alt=\"{}\"", escape_attr(&alt))
    };
    format!(
        "<img{BREAK}src=\"{}\"{}{}{alt_attr}{BREAK}/>",
        escape_attr(&target.url),
        title_attr(&target.title),
        render_attr(attr, AttrOrder::Standard),
    )
}

/// Whether a figure was synthesized from a lone captioned image: its body is a single image whose
/// alt text is the caption verbatim. Such a caption is marked `aria-hidden="true"`
/// because a screen reader already announces the duplicated alt text.
fn is_implicit_figure(caption: &Caption, blocks: &[Block]) -> bool {
    let [Block::Plain(plain)] = blocks else {
        return false;
    };
    let [Inline::Image(_, alt, _)] = plain.as_slice() else {
        return false;
    };
    matches!(caption.long.as_slice(), [Block::Plain(cap)] if cap == alt)
}

fn has_explicit_widths(specs: &[ColSpec]) -> bool {
    specs
        .iter()
        .any(|spec| matches!(spec.width, ColWidth::ColWidth(_)))
}

fn colgroup(specs: &[ColSpec]) -> String {
    if !has_explicit_widths(specs) {
        return String::new();
    }
    let cols: Vec<String> = specs
        .iter()
        .map(|spec| match spec.width {
            ColWidth::ColWidth(width) => {
                format!("<col style=\"width: {}%\" />", width_percent(width))
            }
            ColWidth::ColWidthDefault => "<col />".to_owned(),
        })
        .collect();
    format!("\n<colgroup>\n{}\n</colgroup>", cols.join("\n"))
}

/// The `style="width:N%;"` a table carries when any column has an explicit width: the column
/// fractions summed and floored to a whole percent. Empty when every column uses the default width.
fn table_width_style(specs: &[ColSpec]) -> String {
    if !has_explicit_widths(specs) {
        return String::new();
    }
    let total: f64 = specs
        .iter()
        .map(|spec| match spec.width {
            ColWidth::ColWidth(width) => width,
            ColWidth::ColWidthDefault => 0.0,
        })
        .sum();
    format!("{BREAK}style=\"width:{}%;\"", width_percent(total))
}

/// Append a newline to `text` unless it is empty (used to separate a footnote's leading blocks
/// from the paragraph that carries the backlink).
fn append_trailing_newline(text: &mut String) {
    if !text.is_empty() {
        text.push('\n');
    }
}

fn title_attr(title: &Text) -> String {
    if title.is_empty() {
        String::new()
    } else {
        format!("{BREAK}title=\"{}\"", escape_attr(title))
    }
}

fn header_tag(level: i32) -> String {
    let clamped = level.clamp(1, 6);
    format!("h{clamped}")
}

fn ordered_list_type(style: &ListNumberStyle) -> Option<&'static str> {
    match style {
        ListNumberStyle::DefaultStyle => None,
        ListNumberStyle::Decimal | ListNumberStyle::Example => Some("1"),
        ListNumberStyle::LowerAlpha => Some("a"),
        ListNumberStyle::UpperAlpha => Some("A"),
        ListNumberStyle::LowerRoman => Some("i"),
        ListNumberStyle::UpperRoman => Some("I"),
    }
}

fn alignment_style(align: &Alignment) -> Option<&'static str> {
    match align {
        Alignment::AlignLeft => Some("text-align: left;"),
        Alignment::AlignRight => Some("text-align: right;"),
        Alignment::AlignCenter => Some("text-align: center;"),
        Alignment::AlignDefault => None,
    }
}

/// A column width fraction as a whole-percent integer: the fraction times 100, floored.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn width_percent(fraction: f64) -> u32 {
    (fraction * 100.0).floor() as u32
}

/// Emit a raw-passthrough payload verbatim when its format targets HTML, else drop it (other
/// target formats produce no output in an HTML document).
fn raw_passthrough(format: &str, text: &str) -> String {
    if matches!(format, "html" | "html5" | "html4") {
        protect(text)
    } else {
        String::new()
    }
}

/// Renders an [`Attr`] to its HTML attribute string (with a leading space when non-empty). The
/// field order depends on [`AttrOrder`].
fn render_attr(attr: &Attr, order: AttrOrder) -> String {
    let id = render_id(&attr.id);
    let class = render_class(&attr.classes);
    let keyvals = render_keyvals(&attr.attributes);
    match order {
        AttrOrder::Standard => format!("{id}{class}{keyvals}"),
        AttrOrder::Header => format!("{class}{keyvals}{id}"),
    }
}

fn render_id(id: &Text) -> String {
    if id.is_empty() {
        String::new()
    } else {
        format!("{BREAK}id=\"{}\"", escape_attr(id))
    }
}

fn render_class(classes: &[Text]) -> String {
    if classes.is_empty() {
        String::new()
    } else {
        format!("{BREAK}class=\"{}\"", escape_attr(&classes.join(" ")))
    }
}

fn render_keyvals(attributes: &[(Text, Text)]) -> String {
    let mut out = String::new();
    for (key, value) in attributes {
        let name = if is_known_attribute(key) {
            key.clone()
        } else {
            format!("data-{key}")
        };
        let _ = write!(out, "{BREAK}{name}=\"{}\"", escape_attr(value));
    }
    out
}

/// Replace each [`BREAK`] sentinel with a space or a line break so that inline content fills to
/// [`FILL_COLUMN`] with a greedy fill. A break point becomes a newline
/// when keeping the following chunk on the current line would exceed the fill column; the chunk is
/// the run of literal text up to the next break point or hard newline. Hard newlines (block
/// structure) reset the column. Consecutive break points collapse to one.
fn reflow(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut column = 0usize;
    let mut chars = input.chars();
    while let Some(current) = chars.next() {
        match current {
            '\n' => {
                out.push('\n');
                column = 0;
            }
            BREAK => {
                while chars.clone().next() == Some(BREAK) {
                    chars.next();
                }
                let mut chunk = 0usize;
                for following in chars.clone() {
                    if following == BREAK || following == '\n' {
                        break;
                    }
                    chunk += char_width(following);
                }
                if column + 1 + chunk > FILL_COLUMN {
                    out.push('\n');
                    column = 0;
                } else {
                    out.push(' ');
                    column += 1;
                }
            }
            other => {
                out.push(other);
                column += char_width(other);
            }
        }
    }
    out
}

/// Display width of a character in columns: zero for
/// combining marks, two for wide and fullwidth East Asian characters, one otherwise.
///
/// This uses a Unicode-category zero-width test, distinct from the range-table measure in
/// [`crate::common`] that the plain and LaTeX writers share.
fn char_width(ch: char) -> usize {
    let code = ch as u32;
    if code < 0x0300 {
        return 1;
    }
    if is_zero_width(ch) {
        return 0;
    }
    if is_wide(code) { 2 } else { 1 }
}

fn is_zero_width(ch: char) -> bool {
    use unicode_general_category::{GeneralCategory, get_general_category};
    matches!(
        get_general_category(ch),
        GeneralCategory::NonspacingMark
            | GeneralCategory::EnclosingMark
            | GeneralCategory::Format
            | GeneralCategory::Control
    )
}

/// Escape `&`, `<`, and `>` to their HTML entities, and additionally `"` when `quotes` is set.
fn escape(text: &str, quotes: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if quotes => out.push_str("&quot;"),
            _ => protect_char(ch, &mut out),
        }
    }
    out
}

/// Encode the assembly sentinels so a literal occurrence in document content survives [`reflow`]
/// unchanged instead of being read as a writer-inserted break; [`restore`] reverses this after
/// reflow runs. Any other character is copied verbatim.
fn protect_char(ch: char, out: &mut String) {
    match ch {
        ESCAPE => {
            out.push(ESCAPE);
            out.push(ESCAPE);
        }
        BREAK => {
            out.push(ESCAPE);
            out.push(BREAK_TAG);
        }
        other => out.push(other),
    }
}

/// Protect already-escaped or raw content (raw HTML passthrough) that bypasses [`escape`].
fn protect(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        protect_char(ch, &mut out);
    }
    out
}

/// Reverse [`protect_char`]: collapse each escape sequence left in the reflowed output back to the
/// literal sentinel it stood for. Writer-inserted breaks are already gone (consumed by [`reflow`]),
/// so every remaining introducer marks protected content.
fn restore(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != ESCAPE {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some(ESCAPE) | None => out.push(ESCAPE),
            Some(BREAK_TAG) => out.push(BREAK),
            Some(other) => {
                out.push(ESCAPE);
                out.push(other);
            }
        }
    }
    out
}

/// Escape running text and inline code, which leave the double quote literal.
fn escape_text(text: &str) -> String {
    escape(text, false)
}

/// Escape an attribute value, where the double quote must be entity-encoded. The same policy applies
/// to a `<pre><code>` block's body.
fn escape_attr(text: &str) -> String {
    escape(text, true)
}
