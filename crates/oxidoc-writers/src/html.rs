//! HTML writer: renders the document model to an html5 fragment.
//!
//! The target is `pandoc -t html5` with syntax highlighting and math rendering neutralized
//! (`--syntax-highlighting=none --mathjax`): code blocks render as a plain `<pre><code>` and math
//! as a MathJax-style `\(…\)` / `\[…\]` passthrough span. Those two subsystems (skylighting,
//! texmath) are deferred (see `docs/plans/slice-1-commonmark-html.md`). Output is a fragment with no
//! trailing newline; the caller appends one.

use std::fmt::Write as _;

use oxidoc_ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Document, Inline, ListAttributes,
    ListNumberStyle, MathType, QuoteType, Row, Table, TableBody, Target, Text,
};
use oxidoc_core::{Result, Writer, WriterOptions};

/// Renders a document to an html5 fragment matching the neutralized reference output.
#[derive(Debug, Default, Clone, Copy)]
pub struct HtmlWriter;

impl Writer for HtmlWriter {
    fn write(&self, document: &Document, _options: &WriterOptions) -> Result<String> {
        let mut state = State::default();
        let mut out = state.blocks(&document.blocks);
        out.push_str(&state.footnote_section());
        let filled = reflow(&out);
        Ok(filled.trim_end_matches('\n').to_owned())
    }
}

/// Column at which the writer wraps filled inline content, matching the reference writer's default.
const FILL_COLUMN: usize = 72;

/// Sentinel marking a breakable inline space while the document is assembled as a flat string.
/// [`reflow`] later turns each into either a single space or a line break to fill to
/// [`FILL_COLUMN`]. Using U+0000 is safe because it never survives into escaped HTML output or
/// well-formed document text (readers replace it with U+FFFD).
const BREAK: char = '\u{0}';

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
    /// Render a block sequence, one block per line.
    fn blocks(&mut self, blocks: &[Block]) -> String {
        let rendered: Vec<String> = blocks.iter().map(|block| self.block(block)).collect();
        rendered.join("\n")
    }

    fn block(&mut self, block: &Block) -> String {
        match block {
            Block::Plain(inlines) => self.inlines(inlines),
            Block::Para(inlines) => format!("<p>{}</p>", self.inlines(inlines)),
            Block::Header(level, attr, inlines) => {
                let tag = header_tag(*level);
                format!(
                    "<{tag}{}>{}</{tag}>",
                    render_attr(attr, AttrOrder::Header),
                    self.inlines(inlines)
                )
            }
            Block::CodeBlock(attr, text) => format!(
                "<pre{}><code>{}</code></pre>",
                render_attr(attr, AttrOrder::Standard),
                escape_code_block(text)
            ),
            Block::RawBlock(format, text) => raw_passthrough(&format.0, text),
            Block::BlockQuote(blocks) => {
                format!("<blockquote>\n{}\n</blockquote>", self.blocks(blocks))
            }
            Block::BulletList(items) => self.bullet_list(items),
            Block::OrderedList(attrs, items) => self.ordered_list(attrs, items),
            Block::DefinitionList(items) => self.definition_list(items),
            Block::Div(attr, blocks) => format!(
                "<div{}>\n{}\n</div>",
                render_attr(attr, AttrOrder::Standard),
                self.blocks(blocks)
            ),
            Block::Figure(attr, caption, blocks) => self.figure(attr, caption, blocks),
            Block::HorizontalRule => "<hr />".to_owned(),
            Block::LineBlock(lines) => self.line_block(lines),
            Block::Table(table) => self.table(table),
        }
    }

    fn bullet_list(&mut self, items: &[Vec<Block>]) -> String {
        let lis = self.list_items(items);
        format!("<ul>\n{}\n</ul>", lis.join("\n"))
    }

    fn ordered_list(&mut self, attrs: &ListAttributes, items: &[Vec<Block>]) -> String {
        let mut open = String::from("<ol");
        if attrs.start != 1 {
            let _ = write!(open, " start=\"{}\"", attrs.start);
        }
        if matches!(attrs.style, ListNumberStyle::Example) {
            open.push_str(" class=\"example\"");
        }
        if let Some(kind) = ordered_list_type(&attrs.style) {
            let _ = write!(open, " type=\"{kind}\"");
        }
        open.push('>');
        let lis = self.list_items(items);
        format!("{open}\n{}\n</ol>", lis.join("\n"))
    }

    /// Render each list item's blocks (newline-joined, no surrounding padding) wrapped in `<li>`.
    fn list_items(&mut self, items: &[Vec<Block>]) -> Vec<String> {
        items
            .iter()
            .map(|item| format!("<li>{}</li>", self.blocks(item)))
            .collect()
    }

    fn definition_list(&mut self, items: &[(Vec<Inline>, Vec<Vec<Block>>)]) -> String {
        let mut parts = Vec::new();
        for (term, definitions) in items {
            parts.push(format!("<dt>{}</dt>", self.inlines(term)));
            for definition in definitions {
                parts.push(format!("<dd>\n{}\n</dd>", self.blocks(definition)));
            }
        }
        format!("<dl>\n{}\n</dl>", parts.join("\n"))
    }

    fn figure(&mut self, attr: &Attr, caption: &Caption, blocks: &[Block]) -> String {
        let body = self.blocks(blocks);
        let caption_html = if caption.long.is_empty() {
            String::new()
        } else {
            let hidden = if is_implicit_figure(caption, blocks) {
                " aria-hidden=\"true\""
            } else {
                ""
            };
            format!(
                "\n<figcaption{hidden}>{}</figcaption>",
                self.blocks(&caption.long)
            )
        };
        format!(
            "<figure{}>\n{body}{caption_html}\n</figure>",
            render_attr(attr, AttrOrder::Standard)
        )
    }

    fn line_block(&mut self, lines: &[Vec<Inline>]) -> String {
        let rendered: Vec<String> = lines.iter().map(|line| self.inlines(line)).collect();
        format!(
            "<div class=\"line-block\">{}</div>",
            rendered.join("<br />\n")
        )
    }

    fn table(&mut self, table: &Table) -> String {
        let mut out = format!(
            "<table{}{}>",
            render_attr(&table.attr, AttrOrder::Standard),
            table_width_style(&table.col_specs)
        );
        if !table.caption.long.is_empty() {
            let _ = write!(
                out,
                "\n<caption>{}</caption>",
                self.blocks(&table.caption.long)
            );
        }
        let aligns: Vec<Alignment> = table
            .col_specs
            .iter()
            .map(|spec| spec.align.clone())
            .collect();
        out.push_str(&colgroup(&table.col_specs));
        if !table.head.rows.is_empty() {
            let rows = self.rows(&table.head.rows, &aligns, true);
            let _ = write!(out, "\n<thead>\n{rows}\n</thead>");
        }
        for body in &table.bodies {
            out.push_str(&self.table_body(body, &aligns));
        }
        if !table.foot.rows.is_empty() {
            let rows = self.rows(&table.foot.rows, &aligns, false);
            let _ = write!(out, "\n<tfoot>\n{rows}\n</tfoot>");
        }
        out.push_str("\n</table>");
        out
    }

    fn table_body(&mut self, body: &TableBody, aligns: &[Alignment]) -> String {
        let mut rows = self.rows_vec(&body.head, aligns, true);
        rows.extend(self.rows_vec(&body.body, aligns, false));
        format!("\n<tbody>\n{}\n</tbody>", rows.join("\n"))
    }

    fn rows(&mut self, rows: &[Row], aligns: &[Alignment], header: bool) -> String {
        self.rows_vec(rows, aligns, header).join("\n")
    }

    fn rows_vec(&mut self, rows: &[Row], aligns: &[Alignment], header: bool) -> Vec<String> {
        rows.iter()
            .map(|row| {
                let cells: Vec<String> = row
                    .cells
                    .iter()
                    .enumerate()
                    .map(|(index, cell)| self.cell(cell, aligns.get(index), header))
                    .collect();
                format!("<tr>\n{}\n</tr>", cells.join("\n"))
            })
            .collect()
    }

    fn cell(&mut self, cell: &Cell, col_align: Option<&Alignment>, header: bool) -> String {
        let tag = if header { "th" } else { "td" };
        let effective = match &cell.align {
            Alignment::AlignDefault => col_align.unwrap_or(&Alignment::AlignDefault),
            explicit => explicit,
        };
        let mut attrs = String::new();
        if let Some(style) = alignment_style(effective) {
            let _ = write!(attrs, "{BREAK}style=\"{style}\"");
        }
        if cell.col_span != 1 {
            let _ = write!(attrs, "{BREAK}colspan=\"{}\"", cell.col_span);
        }
        if cell.row_span != 1 {
            let _ = write!(attrs, "{BREAK}rowspan=\"{}\"", cell.row_span);
        }
        format!("<{tag}{attrs}>{}</{tag}>", self.blocks(&cell.content))
    }

    fn inlines(&mut self, inlines: &[Inline]) -> String {
        inlines.iter().map(|inline| self.inline(inline)).collect()
    }

    fn inline(&mut self, inline: &Inline) -> String {
        match inline {
            Inline::Str(text) => escape_text(text),
            Inline::Emph(inlines) => self.wrap("em", inlines),
            Inline::Strong(inlines) => self.wrap("strong", inlines),
            Inline::Strikeout(inlines) => self.wrap("del", inlines),
            Inline::Superscript(inlines) => self.wrap("sup", inlines),
            Inline::Subscript(inlines) => self.wrap("sub", inlines),
            Inline::Underline(inlines) => self.wrap("u", inlines),
            Inline::SmallCaps(inlines) => {
                format!("<span class=\"smallcaps\">{}</span>", self.inlines(inlines))
            }
            Inline::Quoted(kind, inlines) => {
                let (open, close) = match kind {
                    QuoteType::SingleQuote => ('\u{2018}', '\u{2019}'),
                    QuoteType::DoubleQuote => ('\u{201c}', '\u{201d}'),
                };
                format!("{open}{}{close}", self.inlines(inlines))
            }
            Inline::Code(attr, text) => format!(
                "<code{}>{}</code>",
                render_attr(attr, AttrOrder::Standard),
                escape_text(text)
            ),
            Inline::Space | Inline::SoftBreak => BREAK.to_string(),
            Inline::LineBreak => "<br />\n".to_owned(),
            Inline::Math(kind, text) => {
                let (class, open, close) = match kind {
                    MathType::InlineMath => ("inline", "\\(", "\\)"),
                    MathType::DisplayMath => ("display", "\\[", "\\]"),
                };
                format!(
                    "<span class=\"math {class}\">{open}{}{close}</span>",
                    escape_text(text)
                )
            }
            Inline::RawInline(format, text) => raw_passthrough(&format.0, text),
            Inline::Link(attr, inlines, target) => self.link(attr, inlines, target),
            Inline::Image(attr, inlines, target) => image(attr, inlines, target),
            Inline::Span(attr, inlines) => format!(
                "<span{}>{}</span>",
                render_attr(attr, AttrOrder::Standard),
                self.inlines(inlines)
            ),
            Inline::Cite(citations, inlines) => {
                let ids: Vec<&str> = citations
                    .iter()
                    .map(|citation| citation.id.as_str())
                    .collect();
                format!(
                    "<span class=\"citation\" data-cites=\"{}\">{}</span>",
                    escape_attr(&ids.join(" ")),
                    self.inlines(inlines)
                )
            }
            Inline::Note(blocks) => self.note(blocks),
        }
    }

    fn wrap(&mut self, tag: &str, inlines: &[Inline]) -> String {
        format!("<{tag}>{}</{tag}>", self.inlines(inlines))
    }

    fn link(&mut self, attr: &Attr, inlines: &[Inline], target: &Target) -> String {
        format!(
            "<a{BREAK}href=\"{}\"{}{}>{}</a>",
            escape_attr(&target.url),
            render_attr(attr, AttrOrder::Standard),
            title_attr(&target.title),
            self.inlines(inlines)
        )
    }

    fn note(&mut self, blocks: &[Block]) -> String {
        let number = self.footnotes.len() + 1;
        let backlink = format!(
            "<a{BREAK}href=\"#fnref{number}\"{BREAK}class=\"footnote-back\"{BREAK}role=\"doc-backlink\">\u{21a9}\u{fe0e}</a>"
        );
        let body = self.note_body(blocks, &backlink);
        self.footnotes
            .push(format!("<li{BREAK}id=\"fn{number}\">{body}</li>"));
        format!(
            "<a{BREAK}href=\"#fn{number}\"{BREAK}class=\"footnote-ref\"{BREAK}id=\"fnref{number}\"{BREAK}role=\"doc-noteref\"><sup>{number}</sup></a>"
        )
    }

    /// Render a footnote's blocks, appending the backlink inside the final paragraph when the last
    /// block is one, else as a bare trailing element (an unwrapped `Plain`) of its own.
    fn note_body(&mut self, blocks: &[Block], backlink: &str) -> String {
        if let Some((Block::Para(inlines), rest)) = blocks.split_last() {
            let head = with_trailing_newline(self.blocks(rest));
            format!("{head}<p>{}{backlink}</p>", self.inlines(inlines))
        } else {
            let head = with_trailing_newline(self.blocks(blocks));
            format!("{head}{backlink}")
        }
    }

    fn footnote_section(&self) -> String {
        if self.footnotes.is_empty() {
            return String::new();
        }
        format!(
            "\n<section{BREAK}id=\"footnotes\"{BREAK}class=\"footnotes footnotes-end-of-document\"{BREAK}role=\"doc-endnotes\">\n<hr />\n<ol>\n{}\n</ol>\n</section>",
            self.footnotes.join("\n")
        )
    }
}

fn image(attr: &Attr, inlines: &[Inline], target: &Target) -> String {
    let alt = inlines_to_plain(inlines);
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
/// alt text is the caption verbatim. The reference writer marks such a caption `aria-hidden="true"`
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
fn with_trailing_newline(mut text: String) -> String {
    if !text.is_empty() {
        text.push('\n');
    }
    text
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

/// A column width fraction as the whole-percent integer the reference writer emits: the fraction
/// times 100, floored.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn width_percent(fraction: f64) -> u32 {
    (fraction * 100.0).floor() as u32
}

/// Emit a raw-passthrough payload verbatim when its format targets HTML, else drop it (other
/// target formats produce no output in an HTML document).
fn raw_passthrough(format: &str, text: &str) -> String {
    if matches!(format, "html" | "html5" | "html4") {
        text.to_owned()
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

fn is_known_attribute(name: &str) -> bool {
    name.starts_with("data-")
        || name.starts_with("aria-")
        || matches!(name, "epub:type" | "xml:lang" | "xmlns")
        || HTML_ATTRIBUTES.contains(&name)
}

/// HTML attribute names the reference writer emits verbatim; any other key/value attribute is
/// `data-` prefixed. Derived empirically from the pinned binary (not from upstream source).
const HTML_ATTRIBUTES: &[&str] = &[
    "abbr",
    "accept",
    "accept-charset",
    "accesskey",
    "action",
    "allow",
    "alt",
    "async",
    "autocapitalize",
    "autocomplete",
    "autofocus",
    "autoplay",
    "charset",
    "checked",
    "cite",
    "class",
    "cols",
    "colspan",
    "content",
    "contenteditable",
    "controls",
    "coords",
    "crossorigin",
    "data",
    "datetime",
    "decoding",
    "default",
    "defer",
    "dir",
    "dirname",
    "disabled",
    "download",
    "draggable",
    "enctype",
    "enterkeyhint",
    "for",
    "form",
    "formaction",
    "formenctype",
    "formmethod",
    "formnovalidate",
    "formtarget",
    "headers",
    "height",
    "hidden",
    "high",
    "href",
    "hreflang",
    "id",
    "inputmode",
    "integrity",
    "is",
    "ismap",
    "itemid",
    "itemprop",
    "itemref",
    "itemscope",
    "itemtype",
    "kind",
    "lang",
    "list",
    "loading",
    "loop",
    "low",
    "max",
    "maxlength",
    "media",
    "method",
    "min",
    "minlength",
    "multiple",
    "muted",
    "name",
    "nonce",
    "novalidate",
    "open",
    "optimum",
    "pattern",
    "ping",
    "placeholder",
    "playsinline",
    "poster",
    "preload",
    "readonly",
    "referrerpolicy",
    "rel",
    "required",
    "reversed",
    "role",
    "rows",
    "rowspan",
    "sandbox",
    "scope",
    "selected",
    "shape",
    "size",
    "sizes",
    "slot",
    "span",
    "spellcheck",
    "src",
    "srcdoc",
    "srcset",
    "start",
    "step",
    "style",
    "tabindex",
    "target",
    "title",
    "translate",
    "type",
    "usemap",
    "value",
    "width",
    "wrap",
];

/// Replace each [`BREAK`] sentinel with a space or a line break so that inline content fills to
/// [`FILL_COLUMN`], matching the reference writer's greedy fill. A break point becomes a newline
/// when keeping the following chunk on the current line would exceed the fill column; the chunk is
/// the run of literal text up to the next break point or hard newline. Hard newlines (block
/// structure) reset the column. Consecutive break points collapse to one.
fn reflow(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut column = 0usize;
    let mut index = 0usize;
    while let Some(&current) = chars.get(index) {
        match current {
            '\n' => {
                out.push('\n');
                column = 0;
                index += 1;
            }
            BREAK => {
                while matches!(chars.get(index), Some(&BREAK)) {
                    index += 1;
                }
                let mut chunk = 0usize;
                let mut lookahead = index;
                while let Some(&following) = chars.get(lookahead) {
                    if following == BREAK || following == '\n' {
                        break;
                    }
                    chunk += char_width(following);
                    lookahead += 1;
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
                index += 1;
            }
        }
    }
    out
}

/// Display width of a character in columns, matching the reference writer's measure: zero for
/// combining marks, two for wide and fullwidth East Asian characters, one otherwise.
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

fn is_wide(code: u32) -> bool {
    matches!(code,
        0x1100..=0x115F
        | 0x2329 | 0x232A
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xA960..=0xA97F
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE10..=0xFE19
        | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1B000..=0x1B2FF
        | 0x1F200..=0x1F2FF
        | 0x1F300..=0x1F64F
        | 0x1F900..=0x1F9FF
        | 0x20000..=0x3FFFD
    )
}

fn escape_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
    out
}

/// Escape a code block's text. Unlike inline text and inline code, the reference writer also
/// escapes the double quote inside a `<pre><code>` block.
fn escape_code_block(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
    out
}

fn escape_attr(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
    out
}

/// The plain-text projection of an inline sequence, used for an image's `alt` attribute: textual
/// content is kept, formatting wrappers are flattened, and breaks become spaces.
fn inlines_to_plain(inlines: &[Inline]) -> String {
    let mut out = String::new();
    for inline in inlines {
        match inline {
            Inline::Str(text) | Inline::Code(_, text) | Inline::Math(_, text) => {
                out.push_str(text);
            }
            Inline::Space | Inline::SoftBreak | Inline::LineBreak => out.push(' '),
            Inline::Emph(xs)
            | Inline::Strong(xs)
            | Inline::Strikeout(xs)
            | Inline::Superscript(xs)
            | Inline::Subscript(xs)
            | Inline::Underline(xs)
            | Inline::SmallCaps(xs)
            | Inline::Quoted(_, xs)
            | Inline::Span(_, xs)
            | Inline::Link(_, xs, _)
            | Inline::Image(_, xs, _)
            | Inline::Cite(_, xs) => out.push_str(&inlines_to_plain(xs)),
            Inline::RawInline(..) | Inline::Note(_) => {}
        }
    }
    out
}
