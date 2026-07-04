//! HTML writer: renders the document model to an html5 fragment.
//!
//! Syntax highlighting is neutralized: code blocks render as a plain `<pre><code>` (deferred; see
//! `docs/plans/slice-1-commonmark-html.md`). TeX math renders as a `span.math` passthrough whose
//! contents an in-browser typesetting loader reads — wrapped in `\(…\)` / `\[…\]` delimiters for the
//! delimiter-scanning loaders, or as bare TeX for the one that reads the span directly. Output is a
//! fragment with no trailing newline; the caller appends one.

use std::fmt::Write as _;

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Document, Inline, ListAttributes,
    ListNumberStyle, MathType, Row, Table, TableBody, Target, Text, to_plain_text,
};
use carta_core::{MathMethod, MetaVarStyle, Result, WrapMode, Writer, WriterOptions};

use crate::common::{
    FILL_COLUMN, RowSpanGrid, is_known_attribute, is_wide, normalize_image_attr, quote_marks,
};

/// Renders a document to an html5 fragment.
#[derive(Debug, Default, Clone, Copy)]
pub struct HtmlWriter;

impl Writer for HtmlWriter {
    fn write(&self, document: &Document, options: &WriterOptions) -> Result<String> {
        Ok(render_with_flavor(
            &document.blocks,
            Flavor::Html5,
            options.wrap,
            fill_width(options),
            math_output(options),
        ))
    }

    fn default_template(&self) -> Option<&'static str> {
        Some(include_str!("templates/default.html"))
    }

    fn meta_var_style(&self) -> MetaVarStyle {
        MetaVarStyle::Web
    }

    fn numbers_sections_in_body(&self) -> bool {
        true
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
    fn write(&self, document: &Document, options: &WriterOptions) -> Result<String> {
        Ok(render_with_flavor(
            &document.blocks,
            Flavor::Html4,
            options.wrap,
            fill_width(options),
            math_output(options),
        ))
    }

    fn default_template(&self) -> Option<&'static str> {
        Some(include_str!("templates/default.html4"))
    }

    fn meta_var_style(&self) -> MetaVarStyle {
        MetaVarStyle::Web
    }

    fn numbers_sections_in_body(&self) -> bool {
        true
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
    /// The XHTML of an EPUB 3 chapter. Follows [`Flavor::Html5`] but wraps each section in a
    /// `<section>` element (hoisting the heading's identifier onto it), and renders footnotes as
    /// `<aside epub:type="footnote">` collected in an `epub:type="footnotes"` section, with the
    /// reference links carrying `epub:type="noteref"`.
    // Constructed only by the EPUB writer; absent when its feature is sliced out of the build.
    #[allow(dead_code)]
    Epub3,
    /// The XHTML 1.1 of an EPUB 2 chapter. Follows [`Flavor::Html4`] for its presentational
    /// element and attribute choices, but drops any attribute XHTML 1.1 does not admit, wraps each
    /// section in `<div class="section">`, and renders footnotes as `<div>` items carrying a
    /// leading back-reference link.
    // Constructed only by the EPUB writer; absent when its feature is sliced out of the build.
    #[allow(dead_code)]
    Epub2,
}

impl Flavor {
    /// Whether the dialect follows html5's element and attribute choices (as opposed to the
    /// presentational html4 choices).
    fn is_html5_family(self) -> bool {
        matches!(self, Flavor::Html5 | Flavor::Slides | Flavor::Epub3)
    }
}

/// The fragment-target prefix on a footnote link. The slide dialect routes links through the deck's
/// in-page navigation, so its fragments are reached as `#/<id>` rather than `#<id>`.
fn fragment_prefix(flavor: Flavor) -> &'static str {
    match flavor {
        Flavor::Slides => "#/",
        Flavor::Html5 | Flavor::Html4 | Flavor::Epub3 | Flavor::Epub2 => "#",
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
        let mut classes: Vec<carta_ast::Text> =
            class_words.iter().map(|word| (*word).into()).collect();
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
            id: carta_ast::Text::default(),
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
pub(crate) fn fill_slides(assembled: &str, wrap: WrapMode, width: usize) -> String {
    restore(&reflow(assembled, wrap, width))
        .trim_end_matches('\n')
        .to_owned()
}

/// Render a block sequence to an html5 fragment, including the footnote section for any notes the
/// blocks carry, laid out under `wrap` and filled to the default column. The fragment carries no
/// trailing newline.
#[cfg(any(feature = "commonmark", feature = "markdown", feature = "gfm"))]
pub(crate) fn render_fragment(blocks: &[Block], wrap: WrapMode) -> String {
    render_with_flavor(
        blocks,
        Flavor::Html5,
        wrap,
        FILL_COLUMN,
        MathOutput::Delimited,
    )
}

/// Render a chapter's blocks to the XHTML body fragment of an EPUB page. `epub3` selects the EPUB 3
/// dialect (sectioning `<section>` elements, `<aside>` footnotes); otherwise the EPUB 2 dialect is
/// used. Lines are not wrapped, matching a container whose pages are read by software, and the math
/// presentation follows the chosen renderer. The fragment carries no trailing newline.
// Used by the EPUB writer; unreferenced when its feature is sliced out of the build.
#[cfg(feature = "epub")]
pub(crate) fn render_epub_chapter(
    blocks: &[Block],
    epub3: bool,
    options: &WriterOptions,
) -> String {
    let flavor = if epub3 { Flavor::Epub3 } else { Flavor::Epub2 };
    strip_xml_invalid(render_with_flavor(
        blocks,
        flavor,
        WrapMode::None,
        fill_width(options),
        math_output(options),
    ))
}

/// Render an inline sequence to a single line of EPUB XHTML, for a table-of-contents entry or a
/// title-page field. `epub3` selects the EPUB 3 dialect; breakable spaces collapse to ordinary
/// spaces.
// Used by the EPUB writer; unreferenced when its feature is sliced out of the build.
#[cfg(feature = "epub")]
pub(crate) fn render_epub_inlines(inlines: &[Inline], epub3: bool) -> String {
    let flavor = if epub3 { Flavor::Epub3 } else { Flavor::Epub2 };
    let mut state = State {
        flavor,
        ..State::default()
    };
    let mut out = String::new();
    state.inlines(&mut out, inlines);
    // Resolve the assembly sentinels the same way a chapter body does: under `None` reflow collapses
    // each run of breakable spaces to a single ordinary one (never a line break), and restore decodes
    // any content character that was protected from that pass — so a protected control character
    // becomes its literal self and is then dropped as XML-invalid, rather than leaking its escape tag.
    strip_xml_invalid(restore(&reflow(&out, WrapMode::None, FILL_COLUMN)))
}

/// The shared predicate for characters XML 1.0 permits; an EPUB page is XML, so the same rule that
/// keeps the emitter well-formed governs what may survive in a chapter's rendered text.
#[cfg(feature = "epub")]
use carta_core::container::xml::is_xml_char;

/// Drop characters XML forbids from an EPUB page's text. An EPUB chapter is XML, so a stray control
/// character in the source — which no escaping can represent — is removed rather than emitted into a
/// document no reading system can parse. Most text is already clean, so the input is returned intact
/// unless it actually carries a forbidden character.
#[cfg(feature = "epub")]
fn strip_xml_invalid(text: String) -> String {
    if text.chars().all(is_xml_char) {
        text
    } else {
        text.chars().filter(|&ch| is_xml_char(ch)).collect()
    }
}

fn render_with_flavor(
    blocks: &[Block],
    flavor: Flavor,
    wrap: WrapMode,
    width: usize,
    math: MathOutput,
) -> String {
    let mut state = State {
        flavor,
        math,
        ..State::default()
    };
    let mut out = String::new();
    state.blocks(&mut out, blocks);
    state.push_footnote_section(&mut out);
    let filled = restore(&reflow(&out, wrap, width));
    filled.trim_end_matches('\n').to_owned()
}

/// The column an html writer fills to: the requested width, or the default when none is set.
pub(crate) fn fill_width(options: &WriterOptions) -> usize {
    options.columns.unwrap_or(FILL_COLUMN)
}

/// Which math markup an html writer emits for the chosen renderer. KaTeX reads bare TeX from the
/// span; every other method keeps the delimiters.
fn math_output(options: &WriterOptions) -> MathOutput {
    match options.math_method {
        MathMethod::Katex(_) => MathOutput::Raw,
        MathMethod::Plain | MathMethod::MathJax(_) => MathOutput::Delimited,
    }
}

/// Render an inline sequence to a single line of html, with every breakable space emitted as one
/// ordinary space (no reflow). Exposed for writers that embed inline html in an attribute value.
// Used by the outline writer; unreferenced when its feature is sliced out of the build.
#[allow(dead_code)]
pub(crate) fn render_inline_line(inlines: &[Inline]) -> String {
    let mut state = State::default();
    let mut out = String::new();
    state.inlines(&mut out, inlines);
    out.replace([BREAK, SOFT], " ")
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

/// Sentinel marking a soft line break from the source, distinct from the breakable space [`BREAK`].
/// [`reflow`] keeps it as a line break when the document preserves its own breaks and otherwise
/// treats it exactly like [`BREAK`]. As with [`BREAK`], a literal `U+0002` from document content is
/// protected by [`protect_char`] and decoded by [`restore`] so the channel stays unambiguous.
const SOFT: char = '\u{2}';

/// Tag following an [`ESCAPE`] introducer that stands for one content `U+0002`, the counterpart of
/// [`BREAK_TAG`] for the [`SOFT`] sentinel.
const SOFT_TAG: char = '2';

/// Zero-width sentinel ending the breakable chunk that a preceding break point measures. It is never
/// rendered and never becomes a space or newline: [`reflow`] drops it. It guards a preformatted
/// region — a `<pre><code>` body — so the verbatim text after it cannot lengthen the chunk weighed
/// when deciding whether the enclosing start tag wraps. A start tag therefore wraps on its own width,
/// independent of however long the preformatted body that follows runs. As with the other sentinels,
/// a literal `U+0003` from document content is protected by [`protect_char`] and decoded by
/// [`restore`].
const FLUSH: char = '\u{3}';

/// Tag following an [`ESCAPE`] introducer that stands for one content `U+0003`, the counterpart of
/// [`BREAK_TAG`] for the [`FLUSH`] sentinel.
const FLUSH_TAG: char = '3';

/// Where an attribute set is being rendered, which selects the field order. Most elements emit
/// `id`, then `class`, then key/value pairs; headers emit `class`, then key/value pairs, then `id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttrOrder {
    Standard,
    Header,
}

/// How a `span.math` carries its TeX. A typesetting loader that scans the page for delimited math
/// (MathJax) needs the `\(…\)` / `\[…\]` wrappers; KaTeX reads the bare TeX from the span and so
/// takes [`MathOutput::Raw`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum MathOutput {
    /// The TeX is wrapped in `\(…\)` (inline) or `\[…\]` (display). The default.
    #[default]
    Delimited,
    /// The span carries the bare TeX with no delimiters.
    Raw,
}

/// Carries the footnote bodies accumulated while rendering, so notes can be collected inline and
/// emitted as a section at the end of the document.
#[derive(Debug, Default)]
struct State {
    footnotes: Vec<String>,
    in_anchor: bool,
    flavor: Flavor,
    math: MathOutput,
}

/// Class names that select a dedicated HTML element for a [`Inline::Span`] instead of a generic
/// `<span>`. Listed in the precedence used when several apply: the first such class found becomes the
/// outermost element, and any further ones nest inside it.
const SEMANTIC_SPAN_TAGS: [&str; 3] = ["mark", "kbd", "dfn"];

impl State {
    /// Render a block sequence into `out`, one block per line. A block that renders to nothing (such
    /// as an empty paragraph) contributes neither output nor a separating newline.
    fn blocks(&mut self, out: &mut String, blocks: &[Block]) {
        let mut wrote_any = false;
        for block in blocks {
            let checkpoint = out.len();
            if wrote_any {
                out.push('\n');
            }
            let body_start = out.len();
            self.block(out, block);
            if out.len() == body_start {
                out.truncate(checkpoint);
            } else {
                wrote_any = true;
            }
        }
    }

    fn block(&mut self, out: &mut String, block: &Block) {
        match block {
            Block::Plain(inlines) => self.inlines(out, inlines),
            Block::Para(inlines) => {
                if inlines.is_empty() {
                    return;
                }
                out.push_str("<p>");
                self.inlines(out, inlines);
                out.push_str("</p>");
            }
            Block::Header(level, attr, inlines) => {
                let tag = header_tag(*level);
                let rendered = if self.flavor.is_html5_family() {
                    render_attr(attr, AttrOrder::Header, self.flavor)
                } else {
                    render_attr(&heading_attr_html4(attr), AttrOrder::Header, self.flavor)
                };
                let _ = write!(out, "<{tag}{rendered}>");
                self.inlines(out, inlines);
                let _ = write!(out, "</{tag}>");
            }
            Block::CodeBlock(attr, text) => {
                let _ = write!(
                    out,
                    "<pre{}><code>{FLUSH}{}</code></pre>",
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
                // An EPUB 3 chapter promotes a section wrapper (a div marked with the `section`
                // class) to a `<section>` element, consuming that marker class; the heading's
                // identifier already sits on the wrapper. Every other div renders as a `<div>`.
                let section = self.flavor == Flavor::Epub3
                    && attr.classes.iter().any(|class| class == "section");
                if section {
                    let stripped = Attr {
                        id: attr.id.clone(),
                        classes: attr
                            .classes
                            .iter()
                            .filter(|class| class.as_str() != "section")
                            .cloned()
                            .collect(),
                        attributes: attr.attributes.clone(),
                    };
                    let _ = writeln!(
                        out,
                        "<section{}>",
                        render_attr(&stripped, AttrOrder::Standard, self.flavor)
                    );
                    self.blocks(out, blocks);
                    out.push_str("\n</section>");
                } else {
                    let _ = writeln!(
                        out,
                        "<div{}>",
                        render_attr(attr, AttrOrder::Standard, self.flavor)
                    );
                    self.blocks(out, blocks);
                    out.push_str("\n</div>");
                }
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
        if self.flavor.is_html5_family() {
            if let Some(kind) = ordered_list_type(attrs.style) {
                let _ = write!(out, " type=\"{kind}\"");
            }
        } else if let Some(name) = list_style_type(attrs.style) {
            let _ = write!(out, " style=\"list-style-type: {name}\"");
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
        if self.flavor.is_html5_family() {
            self.figure_html5(out, attr, caption, blocks);
        } else {
            self.figure_html4(out, attr, caption, blocks);
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
        if self.flavor.is_html5_family() {
            out.push_str(&cell_attr(&cell.attr, alignment_style(effective)));
        } else {
            out.push_str(&cell_attr_html4(&cell.attr, effective, self.flavor));
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
                let _ = write!(out, "<span{BREAK}class=\"smallcaps\">");
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
            Inline::Space => out.push(BREAK),
            Inline::SoftBreak => out.push(SOFT),
            Inline::LineBreak => out.push_str("<br />\n"),
            Inline::Math(kind, text) => {
                let (class, delimiters) = match kind {
                    MathType::InlineMath => ("inline", ("\\(", "\\)")),
                    MathType::DisplayMath => ("display", ("\\[", "\\]")),
                };
                let (open, close) = match self.math {
                    MathOutput::Delimited => delimiters,
                    MathOutput::Raw => ("", ""),
                };
                let _ = write!(
                    out,
                    "<span{BREAK}class=\"math {class}\">{open}{}{close}</span>",
                    fill_math(text)
                );
            }
            Inline::RawInline(format, text) => out.push_str(&raw_passthrough(&format.0, text)),
            Inline::Link(attr, inlines, target) => self.link(out, attr, inlines, target),
            Inline::Image(attr, inlines, target) => {
                out.push_str(&image(attr, inlines, target, self.flavor));
            }
            Inline::Span(attr, inlines) => self.span(out, attr, inlines),
            Inline::Cite(citations, inlines) => {
                if self.flavor.is_html5_family() {
                    let ids: Vec<&str> = citations
                        .iter()
                        .map(|citation| citation.id.as_str())
                        .collect();
                    let _ = write!(
                        out,
                        "<span class=\"citation\"{BREAK}data-cites=\"{}\">",
                        escape_attr(&ids.join(" "))
                    );
                } else {
                    out.push_str("<span class=\"citation\">");
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
        // The `underline` class wraps the content in a bare `<u>` carrying no attributes; any
        // remaining attributes fall to the enclosing element. Strip it and render the `<u>` as the
        // innermost wrapper below.
        let underline = attr.classes.iter().any(|class| class == "underline");
        let stripped;
        let attr = if underline {
            stripped = Attr {
                id: attr.id.clone(),
                classes: attr
                    .classes
                    .iter()
                    .filter(|class| class.as_str() != "underline")
                    .cloned()
                    .collect(),
                attributes: attr.attributes.clone(),
            };
            &stripped
        } else {
            attr
        };

        let first = attr
            .classes
            .iter()
            .position(|class| SEMANTIC_SPAN_TAGS.contains(&class.as_str()));
        let Some(first) = first else {
            // No dedicated element. A generic `<span>` wraps the content unless the only attribute
            // was the consumed `underline`, leaving nothing to carry — then the bare `<u>` stands
            // alone.
            let bare_underline = underline
                && attr.id.is_empty()
                && attr.classes.is_empty()
                && attr.attributes.is_empty();
            if !bare_underline {
                let _ = write!(
                    out,
                    "<span{}>",
                    render_attr(attr, AttrOrder::Standard, self.flavor)
                );
            }
            if underline {
                out.push_str("<u>");
            }
            self.inlines(out, inlines);
            if underline {
                out.push_str("</u>");
            }
            if !bare_underline {
                out.push_str("</span>");
            }
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
        if underline {
            out.push_str("<u>");
        }
        self.inlines(out, inlines);
        if underline {
            out.push_str("</u>");
        }
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
        match self.flavor {
            Flavor::Epub3 => {
                // The note becomes an `<aside>` gathered into the trailing footnote section; the
                // reference is a plain link (no superscript) tagged as a note reference.
                let mut body = String::new();
                self.blocks(&mut body, blocks);
                self.footnotes.push(format!(
                    "<aside{BREAK}epub:type=\"footnote\"{BREAK}role=\"doc-footnote\"{BREAK}id=\"fn{number}\">\n{body}\n</aside>"
                ));
                let _ = write!(
                    out,
                    "<a{BREAK}href=\"{prefix}fn{number}\"{BREAK}class=\"footnote-ref\"{BREAK}id=\"fnref{number}\"{BREAK}epub:type=\"noteref\"{BREAK}role=\"doc-noteref\">{number}</a>"
                );
            }
            Flavor::Epub2 => {
                // The note becomes a `<div>` whose first paragraph opens with a numbered
                // back-reference link; the reference is a plain link (no superscript).
                let backlink = format!(
                    "<a{BREAK}href=\"{prefix}fnref{number}\"{BREAK}class=\"footnote-back\">{number}</a>. "
                );
                let body = self.note_body_epub2(blocks, &backlink);
                self.footnotes
                    .push(format!("<div{BREAK}id=\"fn{number}\">\n{body}\n</div>"));
                let _ = write!(
                    out,
                    "<a{BREAK}href=\"{prefix}fn{number}\"{BREAK}class=\"footnote-ref\"{BREAK}id=\"fnref{number}\">{number}</a>"
                );
            }
            Flavor::Html5 | Flavor::Slides | Flavor::Html4 => {
                let backlink_role = if self.flavor.is_html5_family() {
                    format!("{BREAK}role=\"doc-backlink\"")
                } else {
                    String::new()
                };
                let backlink = format!(
                    "<a{BREAK}href=\"{prefix}fnref{number}\"{BREAK}class=\"footnote-back\"{backlink_role}>\u{21a9}\u{fe0e}</a>"
                );
                let body = self.note_body(blocks, &backlink);
                self.footnotes
                    .push(format!("<li{BREAK}id=\"fn{number}\">{body}</li>"));
                let ref_role = if self.flavor.is_html5_family() {
                    format!("{BREAK}role=\"doc-noteref\"")
                } else {
                    String::new()
                };
                let _ = write!(
                    out,
                    "<a{BREAK}href=\"{prefix}fn{number}\"{BREAK}class=\"footnote-ref\"{BREAK}id=\"fnref{number}\"{ref_role}><sup>{number}</sup></a>"
                );
            }
        }
    }

    /// Render a footnote's blocks, appending the backlink inline after the final block's content
    /// when that block is a paragraph (wrapped in `<p>`) or an unwrapped `Plain`; for any other
    /// trailing block the backlink follows on its own line. The body is returned as its own value
    /// because notes are gathered for a trailing section.
    fn note_body(&mut self, blocks: &[Block], backlink: &str) -> String {
        let mut body = String::new();
        match blocks.split_last() {
            Some((Block::Para(inlines), rest)) => {
                self.blocks(&mut body, rest);
                append_trailing_newline(&mut body);
                body.push_str("<p>");
                self.inlines(&mut body, inlines);
                body.push_str(backlink);
                body.push_str("</p>");
            }
            Some((Block::Plain(inlines), rest)) => {
                self.blocks(&mut body, rest);
                append_trailing_newline(&mut body);
                self.inlines(&mut body, inlines);
                body.push_str(backlink);
            }
            _ => {
                self.blocks(&mut body, blocks);
                append_trailing_newline(&mut body);
                body.push_str(backlink);
            }
        }
        body
    }

    /// Render an EPUB 2 footnote's blocks, opening the first paragraph (or plain block) with the
    /// numbered back-reference link; any further blocks follow unchanged. A note that does not begin
    /// with a paragraph gets the back-reference on a line of its own.
    fn note_body_epub2(&mut self, blocks: &[Block], backlink: &str) -> String {
        let mut body = String::new();
        match blocks.split_first() {
            Some((Block::Para(inlines), rest)) => {
                body.push_str("<p>");
                body.push_str(backlink);
                self.inlines(&mut body, inlines);
                body.push_str("</p>");
                if !rest.is_empty() {
                    body.push('\n');
                    self.blocks(&mut body, rest);
                }
            }
            Some((Block::Plain(inlines), rest)) => {
                body.push_str(backlink);
                self.inlines(&mut body, inlines);
                if !rest.is_empty() {
                    body.push('\n');
                    self.blocks(&mut body, rest);
                }
            }
            _ => {
                let _ = writeln!(body, "<p>{}</p>", backlink.trim_end());
                self.blocks(&mut body, blocks);
            }
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
            Flavor::Epub3 => {
                let _ = write!(
                    out,
                    "\n<section{BREAK}id=\"footnotes\"{BREAK}class=\"footnotes footnotes-end-of-document\"{BREAK}epub:type=\"footnotes\">\n<hr />\n"
                );
            }
            Flavor::Epub2 => {
                let _ = write!(
                    out,
                    "\n<div{BREAK}class=\"footnotes footnotes-end-of-document\">\n<hr />\n"
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
            Flavor::Epub3 => "\n</section>",
            Flavor::Epub2 => "\n</div>",
        };
        out.push_str(close);
    }
}

fn image(attr: &Attr, inlines: &[Inline], target: &Target, flavor: Flavor) -> String {
    // An EPUB page always carries an `alt` attribute — empty when the image has no description — as
    // its XHTML profile expects; the other html flavors omit it when there is nothing to say.
    let alt_attr = if inlines.is_empty() && !matches!(flavor, Flavor::Epub3 | Flavor::Epub2) {
        String::new()
    } else {
        format!("{BREAK}alt=\"{}\"", escape_attr(&to_plain_text(inlines)))
    };
    let source = match flavor {
        Flavor::Slides => "data-src",
        Flavor::Html5 | Flavor::Html4 | Flavor::Epub3 | Flavor::Epub2 => "src",
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
            ColWidth::ColWidth(width) if flavor.is_html5_family() => {
                format!("<col style=\"width: {}%\" />", width_percent(width))
            }
            ColWidth::ColWidth(width) => format!("<col width=\"{}%\" />", width_percent(width)),
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

fn ordered_list_type(style: ListNumberStyle) -> Option<&'static str> {
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
fn list_style_type(style: ListNumberStyle) -> Option<&'static str> {
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

/// The presentational dimension attributes HTML4 admits on the elements that carry them — an image,
/// a table cell or column: a pixel `width` or `height`. Percentage and length dimensions fold into a
/// `style` declaration upstream, so only bare pixel counts reach the attribute renderer, where the
/// strict XHTML 1.1 dialect would otherwise drop them as unknown.
fn is_html4_dimension_attribute(key: &str) -> bool {
    matches!(key, "width" | "height")
}

/// Render a table cell's attributes for the HTML4 dialect: id, class, an explicit `align="…"`
/// attribute for the effective alignment, then the cell's own key/value pairs verbatim.
fn cell_attr_html4(attr: &Attr, align: &Alignment, flavor: Flavor) -> String {
    let id = render_id(&attr.id);
    let class = render_class(&attr.classes);
    let align_attr = match alignment_word(align) {
        Some(word) => format!("{BREAK}align=\"{word}\""),
        None => String::new(),
    };
    let keyvals = render_keyvals(&attr.attributes, flavor);
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
                key.to_string()
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
/// through under a `data-` prefix; in html4 it is emitted by its bare name. The EPUB 2 dialect
/// targets XHTML 1.1, which admits no such extension attributes, so any key that is not a universal
/// html4 attribute is dropped rather than carried through.
fn render_keyvals(attributes: &[(Text, Text)], flavor: Flavor) -> String {
    let mut out = String::new();
    for (key, value) in attributes {
        if key.is_empty() {
            continue;
        }
        let name = match flavor {
            Flavor::Html5 | Flavor::Slides | Flavor::Epub3 if !is_known_attribute(key) => {
                format!("data-{key}")
            }
            Flavor::Epub2
                if !is_html4_universal_attribute(key) && !is_html4_dimension_attribute(key) =>
            {
                continue;
            }
            _ => key.to_string(),
        };
        let _ = write!(out, "{BREAK}{name}=\"{}\"", escape_attr(value));
    }
    out
}

/// Resolve the break sentinels in an assembled fragment under the document's wrap mode.
///
/// Under [`WrapMode::Auto`] inline content fills to `width` columns with a greedy fill: a break point
/// ([`BREAK`] or [`SOFT`]) becomes a newline when keeping the following chunk on the current line
/// would exceed the fill column, where the chunk is the run of literal text up to the next break
/// point or hard newline. Under [`WrapMode::None`] no break point ever becomes a newline — every one
/// is a space. Under [`WrapMode::Preserve`] a [`SOFT`] (a soft break from the source) becomes a
/// newline while a [`BREAK`] (a breakable space) stays a space, and lines are not reflowed. Hard
/// newlines (block structure) always reset the column; consecutive break points collapse to one.
fn reflow(input: &str, wrap: WrapMode, width: usize) -> String {
    let mut out = String::with_capacity(input.len());
    let mut column = 0usize;
    let mut chars = input.chars();
    while let Some(current) = chars.next() {
        match current {
            '\n' => {
                out.push('\n');
                column = 0;
            }
            FLUSH => {}
            BREAK | SOFT => match wrap {
                // A run of break points is a single reflow decision: the line breaks only when the
                // next chunk (the literal text up to the following break point or hard newline)
                // would overflow the fill column.
                WrapMode::Auto => {
                    while let Some(BREAK | SOFT) = chars.clone().next() {
                        chars.next();
                    }
                    let mut chunk = 0usize;
                    for following in chars.clone() {
                        if following == BREAK
                            || following == SOFT
                            || following == '\n'
                            || following == FLUSH
                        {
                            break;
                        }
                        chunk += char_width(following);
                    }
                    if column + 1 + chunk > width {
                        out.push('\n');
                        column = 0;
                    } else {
                        out.push(' ');
                        column += 1;
                    }
                }
                // Without wrapping a run of break points still collapses to a single space: two
                // spaces left around a vanished inline — a dropped foreign raw inline, say — read as
                // one, the way inter-word spacing always does.
                WrapMode::None => {
                    while let Some(BREAK | SOFT) = chars.clone().next() {
                        chars.next();
                    }
                    out.push(' ');
                    column += 1;
                }
                // Under Preserve each break point stands on its own — a source soft break starts a
                // fresh line, and every other break point is a literal space — so adjacent ones are
                // not merged.
                WrapMode::Preserve if current == SOFT => {
                    out.push('\n');
                    column = 0;
                }
                WrapMode::Preserve => {
                    out.push(' ');
                    column += 1;
                }
            },
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

/// Escape `&`, `<`, and `>` to their HTML entities, and additionally `"` when `double_quote` and `'`
/// when `single_quote` is set.
fn escape(text: &str, double_quote: bool, single_quote: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if double_quote => out.push_str("&quot;"),
            '\'' if single_quote => out.push_str("&#39;"),
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
        SOFT => {
            out.push(ESCAPE);
            out.push(SOFT_TAG);
        }
        FLUSH => {
            out.push(ESCAPE);
            out.push(FLUSH_TAG);
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
            Some(SOFT_TAG) => out.push(SOFT),
            Some(FLUSH_TAG) => out.push(FLUSH),
            Some(other) => {
                out.push(ESCAPE);
                out.push(other);
            }
        }
    }
    out
}

/// Escape running text and inline code, which leave both quote characters literal.
fn escape_text(text: &str) -> String {
    escape(text, false, false)
}

/// Escape an attribute value, where both quote characters must be entity-encoded. The same policy
/// applies to a `<pre><code>` block's body.
fn escape_attr(text: &str) -> String {
    escape(text, true, true)
}

/// Escape the body of a math span, where both quote characters must be entity-encoded so the verbatim
/// formula survives intact for the math renderer.
fn escape_math(text: &str) -> String {
    escape(text, true, true)
}

/// Escape a math span's body and turn its spaces into break points, so a long formula wraps at the
/// fill column the way running text does rather than overflowing the line.
fn fill_math(text: &str) -> String {
    escape_math(text)
        .chars()
        .map(|ch| if ch == ' ' { BREAK } else { ch })
        .collect()
}

#[cfg(test)]
mod escaping_tests {
    use super::{escape_attr, escape_text};

    #[test]
    fn attribute_values_and_code_block_bodies_entity_encode_both_quotes() {
        assert_eq!(escape_attr("a\"b'c<&>"), "a&quot;b&#39;c&lt;&amp;&gt;");
    }

    #[test]
    fn running_text_and_inline_code_keep_both_quotes_literal() {
        assert_eq!(escape_text("a\"b'c<&>"), "a\"b'c&lt;&amp;&gt;");
    }
}

#[cfg(all(test, feature = "epub"))]
mod tests {
    use super::{is_xml_char, strip_xml_invalid};

    #[test]
    fn strips_forbidden_c0_controls_and_keeps_whitespace() {
        // NUL, start-of-heading, bell and unit-separator are forbidden in XML and are dropped; tab,
        // newline and carriage return are the permitted controls and survive.
        let input = String::from("a\u{0}b\u{1}\u{7}c\u{1f}\td\r\ne");
        assert_eq!(strip_xml_invalid(input), "abc\td\r\ne");
    }

    #[test]
    fn returns_clean_text_unchanged() {
        let input = String::from("plain text with unicode \u{2603} and a sum \u{2211}");
        assert_eq!(strip_xml_invalid(input.clone()), input);
    }

    #[test]
    fn classifies_boundary_code_points() {
        for forbidden in [
            '\u{0}', '\u{8}', '\u{b}', '\u{c}', '\u{1f}', '\u{fffe}', '\u{ffff}',
        ] {
            assert!(!is_xml_char(forbidden), "{forbidden:?} must be rejected");
        }
        for allowed in ['\t', '\n', '\r', ' ', 'a', '\u{fffd}', '\u{10000}'] {
            assert!(is_xml_char(allowed), "{allowed:?} must be accepted");
        }
    }
}
