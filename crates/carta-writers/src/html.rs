//! HTML writer: renders the document model to an html5 fragment.
//!
//! Syntax highlighting and TeX math rendering are neutralized: code blocks render as a plain
//! `<pre><code>` and math as a MathJax-style `\(…\)` / `\[…\]` passthrough span. Those two
//! subsystems are deferred (see `docs/plans/slice-1-commonmark-html.md`). Output is a fragment with
//! no trailing newline; the caller appends one.

use std::fmt::Write as _;

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Document, Inline, ListAttributes,
    ListNumberStyle, MathType, Row, Table, TableBody, Target, Text, to_plain_text,
};
use carta_core::{Result, Writer, WriterOptions};

use crate::common::{
    FILL_COLUMN, RowSpanGrid, is_known_attribute, is_wide, normalize_image_attr, quote_marks,
};

/// Renders a document to an html5 fragment.
#[derive(Debug, Default, Clone, Copy)]
pub struct HtmlWriter;

impl Writer for HtmlWriter {
    fn write(&self, document: &Document, _options: &WriterOptions) -> Result<String> {
        Ok(render_fragment(&document.blocks))
    }

    fn default_template(&self) -> Option<&'static str> {
        Some(include_str!("templates/default.html"))
    }
}

/// Renders a document to an html4 fragment. The html4 dialect uses presentational attributes
/// (`align`, `width`) where html5 uses inline `style`, wraps figures in `<div class="float">`
/// rather than `<figure>`, groups footnotes in a `<div>` rather than a `<section>`, drops the
/// ARIA document roles, and emits non-standard attributes by their bare name rather than under a
/// `data-` prefix.
#[derive(Debug, Default, Clone, Copy)]
pub struct Html4Writer;

impl Writer for Html4Writer {
    fn write(&self, document: &Document, _options: &WriterOptions) -> Result<String> {
        Ok(render_with_flavor(&document.blocks, Flavor::Html4))
    }

    fn default_template(&self) -> Option<&'static str> {
        Some(include_str!("templates/default.html4"))
    }
}

/// The HTML dialect a render targets. They differ in a handful of element and attribute choices;
/// every divergence is keyed off this value.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Flavor {
    #[default]
    Html5,
    Html4,
    /// The dialect of an html5 slide deck: identical to [`Flavor::Html5`] except that footnote
    /// links carry the deck's in-page navigation prefix on their fragment targets.
    // Constructed only by the slide writer; absent when its feature is sliced out of the build.
    #[allow(dead_code)]
    Slides,
}

/// The fragment-target prefix on a footnote link. The slide dialect routes links through the deck's
/// in-page navigation, so its fragments are reached as `#/<id>` rather than `#<id>`.
fn fragment_prefix(flavor: Flavor) -> &'static str {
    match flavor {
        Flavor::Slides => "#/",
        Flavor::Html5 | Flavor::Html4 => "#",
    }
}

/// Drives html5 block rendering across a slide deck's frames, gathering every frame's footnotes into
/// one accumulator so they can be emitted as a single trailing section. Each method returns an
/// unreflowed fragment carrying the break sentinels; the caller assembles the slide structure around
/// the fragments and then calls [`fill_slides`] once over the whole document.
// Used by the slide writer; unreferenced when its feature is sliced out of the build.
#[allow(dead_code)]
pub(crate) struct SlideRenderer {
    state: State,
}

#[allow(dead_code)]
impl SlideRenderer {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            state: State {
                flavor: Flavor::Slides,
                ..State::default()
            },
        }
    }

    /// The open tag of a slide's `<section>`: the header's `id`, then a `class` whose value is the
    /// given class words followed by the header's own classes, then the header's key/value pairs. A
    /// titleless slide passes an empty `attr`, yielding the class words alone.
    #[must_use]
    pub(crate) fn section_open(attr: &Attr, class_words: &[&str]) -> String {
        let mut classes: Vec<String> = class_words.iter().map(|word| (*word).to_owned()).collect();
        classes.extend(attr.classes.iter().cloned());
        let mut tag = String::from("<section");
        tag.push_str(&render_id(&attr.id));
        tag.push_str(&render_class(&classes));
        tag.push_str(&render_keyvals(&attr.attributes, Flavor::Slides));
        tag.push('>');
        tag
    }

    /// A slide title rendered as its heading element with the header's classes and key/value pairs
    /// but without its `id` (the `id` belongs to the enclosing `<section>`).
    #[must_use]
    pub(crate) fn title(&mut self, level: i32, attr: &Attr, inlines: &[Inline]) -> String {
        let tag = header_tag(level);
        let titleless = Attr {
            id: String::new(),
            classes: attr.classes.clone(),
            attributes: attr.attributes.clone(),
        };
        let mut out = format!(
            "<{tag}{}>",
            render_attr(&titleless, AttrOrder::Header, Flavor::Slides)
        );
        self.state.inlines(&mut out, inlines);
        let _ = write!(out, "</{tag}>");
        out
    }

    /// A frame body rendered as an html5 fragment; any footnotes it carries join the accumulator.
    #[must_use]
    pub(crate) fn body(&mut self, blocks: &[Block]) -> String {
        let mut out = String::new();
        self.state.blocks(&mut out, blocks);
        out
    }

    /// The accumulated footnotes as a trailing `<section>`, or `None` when no note was rendered.
    #[must_use]
    pub(crate) fn footnote_section(&self) -> Option<String> {
        let mut out = String::new();
        self.state.push_footnote_section(&mut out);
        // `push_footnote_section` opens with a leading newline that joins the section to preceding
        // content; the deck supplies its own separator, so drop it.
        let trimmed = out.trim_start_matches('\n');
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        }
    }
}

/// Resolve the break sentinels in an assembled slide document, filling inline runs to the fill
/// column, and trim the trailing newlines. Counterpart to the per-frame rendering on
/// [`SlideRenderer`].
// Used by the slide writer; unreferenced when its feature is sliced out of the build.
#[allow(dead_code)]
#[must_use]
pub(crate) fn fill_slides(assembled: &str) -> String {
    restore(&reflow(assembled))
        .trim_end_matches('\n')
        .to_owned()
}

/// Render a block sequence to an html5 fragment, including the footnote section for any notes the
/// blocks carry. The fragment carries no trailing newline.
pub(crate) fn render_fragment(blocks: &[Block]) -> String {
    render_with_flavor(blocks, Flavor::Html5)
}

fn render_with_flavor(blocks: &[Block], flavor: Flavor) -> String {
    let mut state = State {
        flavor,
        ..State::default()
    };
    let mut out = String::new();
    state.blocks(&mut out, blocks);
    state.push_footnote_section(&mut out);
    let filled = restore(&reflow(&out));
    filled.trim_end_matches('\n').to_owned()
}

/// Render an inline sequence to a single line of html, with every breakable space emitted as one
/// ordinary space (no reflow). Exposed for writers that embed inline html in an attribute value.
// Used by the outline writer; unreferenced when its feature is sliced out of the build.
#[allow(dead_code)]
pub(crate) fn render_inline_line(inlines: &[Inline]) -> String {
    let mut state = State::default();
    let mut out = String::new();
    state.inlines(&mut out, inlines);
    out.replace(BREAK, " ")
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
    in_anchor: bool,
    flavor: Flavor,
}

/// Class names that select a dedicated HTML element for a [`Inline::Span`] instead of a generic
/// `<span>`. Listed in the precedence used when several apply: the first such class found becomes the
/// outermost element, and any further ones nest inside it.
const SEMANTIC_SPAN_TAGS: [&str; 3] = ["mark", "kbd", "dfn"];

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
                let rendered = match self.flavor {
                    Flavor::Html5 | Flavor::Slides => {
                        render_attr(attr, AttrOrder::Header, self.flavor)
                    }
                    Flavor::Html4 => {
                        render_attr(&heading_attr_html4(attr), AttrOrder::Header, self.flavor)
                    }
                };
                let _ = write!(out, "<{tag}{rendered}>");
                self.inlines(out, inlines);
                let _ = write!(out, "</{tag}>");
            }
            Block::CodeBlock(attr, text) => {
                let _ = write!(
                    out,
                    "<pre{}><code>{}</code></pre>",
                    render_attr(attr, AttrOrder::Standard, self.flavor),
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
                let _ = writeln!(
                    out,
                    "<div{}>",
                    render_attr(attr, AttrOrder::Standard, self.flavor)
                );
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
        if !items.is_empty() && items.iter().all(|item| checkbox_state(item).is_some()) {
            out.push_str("<ul class=\"task-list\">\n");
        } else {
            out.push_str("<ul>\n");
        }
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
        match self.flavor {
            Flavor::Html5 | Flavor::Slides => {
                if let Some(kind) = ordered_list_type(&attrs.style) {
                    let _ = write!(out, " type=\"{kind}\"");
                }
            }
            Flavor::Html4 => {
                if let Some(name) = list_style_type(&attrs.style) {
                    let _ = write!(out, " style=\"list-style-type: {name}\"");
                }
            }
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
            match checkbox_state(item) {
                Some(checked) => self.checkbox_item(out, item, checked),
                None => self.blocks(out, item),
            }
            out.push_str("</li>");
        }
    }

    fn checkbox_item(&mut self, out: &mut String, item: &[Block], checked: bool) {
        let input = if checked {
            "<label><input type=\"checkbox\" checked=\"\" />"
        } else {
            "<label><input type=\"checkbox\" />"
        };
        for (index, block) in item.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            match (index, block) {
                (0, Block::Plain(inlines)) => {
                    out.push_str(input);
                    self.inlines(out, inlines.get(2..).unwrap_or_default());
                    out.push_str("</label>");
                }
                (0, Block::Para(inlines)) => {
                    out.push_str("<p>");
                    out.push_str(input);
                    self.inlines(out, inlines.get(2..).unwrap_or_default());
                    out.push_str("</label></p>");
                }
                _ => self.block(out, block),
            }
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
        match self.flavor {
            Flavor::Html5 | Flavor::Slides => self.figure_html5(out, attr, caption, blocks),
            Flavor::Html4 => self.figure_html4(out, attr, caption, blocks),
        }
    }

    fn figure_html5(&mut self, out: &mut String, attr: &Attr, caption: &Caption, blocks: &[Block]) {
        let _ = writeln!(
            out,
            "<figure{}>",
            render_attr(attr, AttrOrder::Standard, self.flavor)
        );
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

    fn figure_html4(&mut self, out: &mut String, attr: &Attr, caption: &Caption, blocks: &[Block]) {
        let _ = writeln!(
            out,
            "<div class=\"float\"{}>",
            render_attr(attr, AttrOrder::Standard, self.flavor)
        );
        self.blocks(out, blocks);
        if !caption.long.is_empty() {
            out.push_str("\n<div class=\"figcaption\">");
            self.blocks(out, &caption.long);
            out.push_str("</div>");
        }
        out.push_str("\n</div>");
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
            render_attr(&table.attr, AttrOrder::Standard, self.flavor),
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
        out.push_str(&colgroup(&table.col_specs, self.flavor));
        if !table.head.rows.is_empty() {
            let _ = write!(
                out,
                "\n<thead{}>",
                render_attr(&table.head.attr, AttrOrder::Standard, self.flavor)
            );
            out.push('\n');
            self.rows(out, &table.head.rows, &aligns, true);
            out.push_str("\n</thead>");
        }
        for body in &table.bodies {
            self.table_body(out, body, &aligns);
        }
        if !table.foot.rows.is_empty() {
            // The foot opens directly after `</tbody>`; only a footless body section or a
            // bodiless foot gets its own line.
            if table.bodies.is_empty() {
                out.push('\n');
            }
            let _ = write!(
                out,
                "<tfoot{}>",
                render_attr(&table.foot.attr, AttrOrder::Standard, self.flavor)
            );
            out.push('\n');
            self.rows(out, &table.foot.rows, &aligns, false);
            out.push_str("\n</tfoot>");
        }
        // A table that ends without body rows (no bodies, or a trailing foot) closes after a
        // blank line.
        if table.bodies.is_empty() || !table.foot.rows.is_empty() {
            out.push('\n');
        }
        out.push_str("\n</table>");
    }

    fn table_body(&mut self, out: &mut String, body: &TableBody, aligns: &[Alignment]) {
        let _ = write!(
            out,
            "\n<tbody{}>",
            render_attr(&body.attr, AttrOrder::Standard, self.flavor)
        );
        let mut head_grid = RowSpanGrid::new(aligns.len());
        for row in &body.head {
            out.push('\n');
            self.row(out, row, aligns, true, 0, &mut head_grid);
        }
        // A blank line separates a body's own header rows from the rows that follow.
        if !body.head.is_empty() {
            out.push('\n');
        }
        let mut body_grid = RowSpanGrid::new(aligns.len());
        for row in &body.body {
            out.push('\n');
            self.row(
                out,
                row,
                aligns,
                false,
                body.row_head_columns,
                &mut body_grid,
            );
        }
        out.push_str("\n</tbody>");
    }

    fn rows(&mut self, out: &mut String, rows: &[Row], aligns: &[Alignment], header: bool) {
        let mut grid = RowSpanGrid::new(aligns.len());
        for (index, row) in rows.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            self.row(out, row, aligns, header, 0, &mut grid);
        }
    }

    fn row(
        &mut self,
        out: &mut String,
        row: &Row,
        aligns: &[Alignment],
        header: bool,
        head_columns: i32,
        grid: &mut RowSpanGrid,
    ) {
        let _ = write!(
            out,
            "<tr{}>",
            render_attr(&row.attr, AttrOrder::Standard, self.flavor)
        );
        out.push('\n');
        let head_columns = usize::try_from(head_columns).unwrap_or(0);
        for (index, (column, cell)) in grid.place(&row.cells).into_iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            self.cell(
                out,
                cell,
                aligns.get(column),
                header || column < head_columns,
            );
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
        if cell.col_span != 1 {
            let _ = write!(out, "{BREAK}colspan=\"{}\"", cell.col_span);
        }
        if cell.row_span != 1 {
            let _ = write!(out, "{BREAK}rowspan=\"{}\"", cell.row_span);
        }
        match self.flavor {
            Flavor::Html5 | Flavor::Slides => {
                out.push_str(&cell_attr(&cell.attr, alignment_style(effective)));
            }
            Flavor::Html4 => out.push_str(&cell_attr_html4(&cell.attr, effective)),
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
                    render_attr(attr, AttrOrder::Standard, self.flavor),
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
            Inline::Image(attr, inlines, target) => {
                out.push_str(&image(attr, inlines, target, self.flavor));
            }
            Inline::Span(attr, inlines) => self.span(out, attr, inlines),
            Inline::Cite(citations, inlines) => {
                match self.flavor {
                    Flavor::Html5 | Flavor::Slides => {
                        let ids: Vec<&str> = citations
                            .iter()
                            .map(|citation| citation.id.as_str())
                            .collect();
                        let _ = write!(
                            out,
                            "<span class=\"citation\" data-cites=\"{}\">",
                            escape_attr(&ids.join(" "))
                        );
                    }
                    Flavor::Html4 => out.push_str("<span class=\"citation\">"),
                }
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

    /// Render a span. A class naming a dedicated HTML element (see [`SEMANTIC_SPAN_TAGS`]) promotes
    /// the span to that element: the first such class becomes the outermost tag and carries the id,
    /// key/value attributes, and any non-semantic classes following it; further semantic classes
    /// nest inside it as bare elements. Classes preceding the first semantic one are dropped. With no
    /// semantic class the span renders as a generic `<span>`.
    fn span(&mut self, out: &mut String, attr: &Attr, inlines: &[Inline]) {
        let first = attr
            .classes
            .iter()
            .position(|class| SEMANTIC_SPAN_TAGS.contains(&class.as_str()));
        let Some(first) = first else {
            let _ = write!(
                out,
                "<span{}>",
                render_attr(attr, AttrOrder::Standard, self.flavor)
            );
            self.inlines(out, inlines);
            out.push_str("</span>");
            return;
        };
        let mut tags = Vec::new();
        let mut remaining = Vec::new();
        for class in attr.classes.iter().skip(first) {
            if SEMANTIC_SPAN_TAGS.contains(&class.as_str()) {
                tags.push(class.as_str());
            } else {
                remaining.insert(0, class.clone());
            }
        }
        let outer = Attr {
            id: attr.id.clone(),
            classes: remaining,
            attributes: attr.attributes.clone(),
        };
        for (index, tag) in tags.iter().enumerate() {
            if index == 0 {
                let _ = write!(
                    out,
                    "<{tag}{}>",
                    render_attr(&outer, AttrOrder::Standard, self.flavor)
                );
            } else {
                let _ = write!(out, "<{tag}>");
            }
        }
        self.inlines(out, inlines);
        for tag in tags.iter().rev() {
            let _ = write!(out, "</{tag}>");
        }
    }

    fn link(&mut self, out: &mut String, attr: &Attr, inlines: &[Inline], target: &Target) {
        if self.in_anchor {
            let _ = write!(
                out,
                "<span{}>",
                render_attr(attr, AttrOrder::Standard, self.flavor)
            );
            self.inlines(out, inlines);
            out.push_str("</span>");
            return;
        }
        let _ = write!(
            out,
            "<a{BREAK}href=\"{}\"{}{}>",
            escape_attr(&target.url),
            render_attr(attr, AttrOrder::Standard, self.flavor),
            title_attr(&target.title)
        );
        self.in_anchor = true;
        self.inlines(out, inlines);
        self.in_anchor = false;
        out.push_str("</a>");
    }

    fn note(&mut self, out: &mut String, blocks: &[Block]) {
        let number = self.footnotes.len() + 1;
        let prefix = fragment_prefix(self.flavor);
        let backlink_role = match self.flavor {
            Flavor::Html5 | Flavor::Slides => format!("{BREAK}role=\"doc-backlink\""),
            Flavor::Html4 => String::new(),
        };
        let backlink = format!(
            "<a{BREAK}href=\"{prefix}fnref{number}\"{BREAK}class=\"footnote-back\"{backlink_role}>\u{21a9}\u{fe0e}</a>"
        );
        let body = self.note_body(blocks, &backlink);
        self.footnotes
            .push(format!("<li{BREAK}id=\"fn{number}\">{body}</li>"));
        let ref_role = match self.flavor {
            Flavor::Html5 | Flavor::Slides => format!("{BREAK}role=\"doc-noteref\""),
            Flavor::Html4 => String::new(),
        };
        let _ = write!(
            out,
            "<a{BREAK}href=\"{prefix}fn{number}\"{BREAK}class=\"footnote-ref\"{BREAK}id=\"fnref{number}\"{ref_role}><sup>{number}</sup></a>"
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
        match self.flavor {
            Flavor::Html5 | Flavor::Slides => {
                let _ = write!(
                    out,
                    "\n<section{BREAK}id=\"footnotes\"{BREAK}class=\"footnotes footnotes-end-of-document\"{BREAK}role=\"doc-endnotes\">\n<hr />\n<ol>\n"
                );
            }
            Flavor::Html4 => {
                let _ = write!(
                    out,
                    "\n<div{BREAK}class=\"footnotes footnotes-end-of-document\">\n<hr />\n<ol>\n"
                );
            }
        }
        for (index, note) in self.footnotes.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            out.push_str(note);
        }
        let close = match self.flavor {
            Flavor::Html5 | Flavor::Slides => "\n</ol>\n</section>",
            Flavor::Html4 => "\n</ol>\n</div>",
        };
        out.push_str(close);
    }
}

fn image(attr: &Attr, inlines: &[Inline], target: &Target, flavor: Flavor) -> String {
    let alt = to_plain_text(inlines);
    let alt_attr = if alt.is_empty() {
        String::new()
    } else {
        format!("{BREAK}alt=\"{}\"", escape_attr(&alt))
    };
    let source = match flavor {
        Flavor::Slides => "data-src",
        Flavor::Html5 | Flavor::Html4 => "src",
    };
    format!(
        "<img{BREAK}{source}=\"{}\"{}{}{alt_attr}{BREAK}/>",
        escape_attr(&target.url),
        title_attr(&target.title),
        render_attr(&normalize_image_attr(attr), AttrOrder::Standard, flavor),
    )
}

/// Whether a figure's body is a single captioned image whose alt text reads the same as its
/// caption. Such a caption is marked `aria-hidden="true"` so a screen reader does not announce the
/// duplicated text twice. The comparison is on plain text, so markup that leaves the spoken words
/// unchanged (emphasis, say) still counts as a match.
fn is_implicit_figure(caption: &Caption, blocks: &[Block]) -> bool {
    let [Block::Plain(plain)] = blocks else {
        return false;
    };
    let [Inline::Image(_, alt, _)] = plain.as_slice() else {
        return false;
    };
    let [Block::Para(cap) | Block::Plain(cap)] = caption.long.as_slice() else {
        return false;
    };
    carta_ast::to_plain_text(cap) == carta_ast::to_plain_text(alt)
}

/// A list item is a task-list entry when its first block opens with a ballot-box character followed
/// by a space; the boolean reports whether the box is checked.
fn checkbox_state(item: &[Block]) -> Option<bool> {
    let (Block::Plain(inlines) | Block::Para(inlines)) = item.first()? else {
        return None;
    };
    let [Inline::Str(marker), Inline::Space, ..] = inlines.as_slice() else {
        return None;
    };
    match marker.as_str() {
        "\u{2610}" => Some(false),
        "\u{2612}" => Some(true),
        _ => None,
    }
}

fn has_explicit_widths(specs: &[ColSpec]) -> bool {
    specs
        .iter()
        .any(|spec| matches!(spec.width, ColWidth::ColWidth(_)))
}

fn colgroup(specs: &[ColSpec], flavor: Flavor) -> String {
    if !has_explicit_widths(specs) {
        return String::new();
    }
    let cols: Vec<String> = specs
        .iter()
        .map(|spec| match spec.width {
            ColWidth::ColWidth(width) => match flavor {
                Flavor::Html5 | Flavor::Slides => {
                    format!("<col style=\"width: {}%\" />", width_percent(width))
                }
                Flavor::Html4 => format!("<col width=\"{}%\" />", width_percent(width)),
            },
            ColWidth::ColWidthDefault => "<col />".to_owned(),
        })
        .collect();
    format!("\n<colgroup>\n{}\n</colgroup>", cols.join("\n"))
}

/// The `style="width:N%;"` a table carries when its explicit column widths leave it narrower
/// than the page: the column fractions summed and rounded to a whole percent. Empty when every
/// column uses the default width, and also when the fractions already cover the full width.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
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
    if total >= 1.0 {
        return String::new();
    }
    format!(
        "{BREAK}style=\"width:{}%;\"",
        (total * 100.0).round() as u32
    )
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

/// The CSS `list-style-type` name for an ordered list's numbering, or `None` for the default style
/// (which carries no explicit list-style declaration).
fn list_style_type(style: &ListNumberStyle) -> Option<&'static str> {
    match style {
        ListNumberStyle::DefaultStyle => None,
        ListNumberStyle::Decimal | ListNumberStyle::Example => Some("decimal"),
        ListNumberStyle::LowerAlpha => Some("lower-alpha"),
        ListNumberStyle::UpperAlpha => Some("upper-alpha"),
        ListNumberStyle::LowerRoman => Some("lower-roman"),
        ListNumberStyle::UpperRoman => Some("upper-roman"),
    }
}

/// The `align="…"` attribute value for a cell's effective alignment, or `None` for the default
/// (which carries no alignment attribute).
fn alignment_word(align: &Alignment) -> Option<&'static str> {
    match align {
        Alignment::AlignLeft => Some("left"),
        Alignment::AlignRight => Some("right"),
        Alignment::AlignCenter => Some("center"),
        Alignment::AlignDefault => None,
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
/// field order depends on [`AttrOrder`]; the spelling of non-standard attribute keys depends on the
/// [`Flavor`].
fn render_attr(attr: &Attr, order: AttrOrder, flavor: Flavor) -> String {
    let id = render_id(&attr.id);
    let class = render_class(&attr.classes);
    let keyvals = render_keyvals(&attr.attributes, flavor);
    match order {
        AttrOrder::Standard => format!("{id}{class}{keyvals}"),
        AttrOrder::Header => format!("{class}{keyvals}{id}"),
    }
}

/// The HTML4-valid universal attributes for a heading element. HTML4 admits only the core, i18n,
/// and presentational attributes plus event handlers on `<hN>`; any other key/value pair is
/// dropped rather than carried through under a `data-` prefix.
fn heading_attr_html4(attr: &Attr) -> Attr {
    let attributes = attr
        .attributes
        .iter()
        .filter(|(key, _)| is_html4_universal_attribute(key))
        .cloned()
        .collect();
    Attr {
        id: attr.id.clone(),
        classes: attr.classes.clone(),
        attributes,
    }
}

/// Whether a key is admissible on any HTML4 element: the core attributes (`style`, `title`, `class`,
/// `id` are handled separately), the i18n attributes, the presentational `align`, and the intrinsic
/// event handlers (`on…`).
fn is_html4_universal_attribute(key: &str) -> bool {
    matches!(key, "style" | "title" | "lang" | "dir" | "align") || key.starts_with("on")
}

/// Render a table cell's attributes for the HTML4 dialect: id, class, an explicit `align="…"`
/// attribute for the effective alignment, then the cell's own key/value pairs verbatim.
fn cell_attr_html4(attr: &Attr, align: &Alignment) -> String {
    let id = render_id(&attr.id);
    let class = render_class(&attr.classes);
    let align_attr = match alignment_word(align) {
        Some(word) => format!("{BREAK}align=\"{word}\""),
        None => String::new(),
    };
    let keyvals = render_keyvals(&attr.attributes, Flavor::Html4);
    format!("{id}{class}{align_attr}{keyvals}")
}

/// Render a table cell's attributes, folding the column's alignment into the `style` declaration.
/// The alignment prefixes any existing `style` value (at that value's position); with no `style`
/// attribute present, an alignment-only `style` is emitted as the first key/value pair, after id and
/// class. With no alignment the attributes render unchanged.
fn cell_attr(attr: &Attr, align_style: Option<&str>) -> String {
    let id = render_id(&attr.id);
    let class = render_class(&attr.classes);
    let Some(align_style) = align_style else {
        return format!(
            "{id}{class}{}",
            render_keyvals(&attr.attributes, Flavor::Html5)
        );
    };
    let mut keyvals = String::new();
    let mut merged = false;
    for (key, value) in &attr.attributes {
        if key.is_empty() {
            continue;
        }
        if key == "style" {
            let combined = combine_style(align_style, value);
            let _ = write!(keyvals, "{BREAK}style=\"{}\"", escape_attr(&combined));
            merged = true;
        } else {
            let name = if is_known_attribute(key) {
                key.clone()
            } else {
                format!("data-{key}")
            };
            let _ = write!(keyvals, "{BREAK}{name}=\"{}\"", escape_attr(value));
        }
    }
    if merged {
        format!("{id}{class}{keyvals}")
    } else {
        format!("{id}{class}{BREAK}style=\"{align_style}\"{keyvals}")
    }
}

/// Prefix a `style` value with an alignment declaration, ensuring the result ends with a semicolon.
fn combine_style(align_style: &str, style: &str) -> String {
    let trimmed = style.trim();
    let suffix = if trimmed.ends_with(';') { "" } else { ";" };
    format!("{align_style} {trimmed}{suffix}")
}

fn render_id(id: &Text) -> String {
    if id.is_empty() {
        String::new()
    } else {
        format!("{BREAK}id=\"{}\"", escape_attr(id))
    }
}

fn render_class(classes: &[Text]) -> String {
    let names: Vec<&str> = classes
        .iter()
        .map(Text::as_str)
        .filter(|class| !class.is_empty())
        .collect();
    if names.is_empty() {
        String::new()
    } else {
        format!("{BREAK}class=\"{}\"", escape_attr(&names.join(" ")))
    }
}

/// Render an attribute set's key/value pairs. In the html5 dialect a non-standard key is carried
/// through under a `data-` prefix; in html4 it is emitted by its bare name.
fn render_keyvals(attributes: &[(Text, Text)], flavor: Flavor) -> String {
    let mut out = String::new();
    for (key, value) in attributes {
        if key.is_empty() {
            continue;
        }
        let name = match flavor {
            Flavor::Html5 | Flavor::Slides if !is_known_attribute(key) => format!("data-{key}"),
            _ => key.clone(),
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

/// Display width of a character in columns: zero for combining marks and control characters, two
/// for wide and fullwidth East Asian characters, one otherwise.
///
/// This uses a Unicode-category zero-width test, distinct from the range-table measure in
/// [`crate::common`] that the plain and LaTeX writers share.
fn char_width(ch: char) -> usize {
    let code = ch as u32;
    if is_zero_width(ch) {
        return 0;
    }
    if code < 0x0300 {
        return 1;
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
