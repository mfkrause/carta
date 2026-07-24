//! Markdown writer engine: renders the document model to a markdown family text format.
//!
//! Every markdown-family dialect shares this engine, parameterized by the `MarkdownConfig` it runs
//! with, chiefly the active [`Extensions`] set. Each construct consults the set to choose its
//! surface: an attribute block on a header, link, image, or span versus a bare or HTML rendering;
//! native subscript/superscript/strikeout versus an HTML tag; fenced versus indented code; fenced
//! divs versus `<div>`; space-aligned, bordered, pipe, or HTML tables; citation syntax versus the
//! display text; dollar, GitHub, or linearized math; raw passthrough with a format tag versus a
//! verbatim or dropped fallback. Smart punctuation is rewritten to ASCII when the `smart` extension
//! is active and emitted as the literal glyph otherwise. Inline content wraps at a fill column of
//! 72. Output carries no trailing newline; the caller appends one.

use carta_ast::{
    Attr, Block, Caption, Document, Format, Inline, ListAttributes, ListNumberDelim,
    ListNumberStyle,
};
use carta_core::{Extension, Extensions, Result, WrapMode, Writer, WriterOptions, presets};

use crate::common::{
    FILL_COLUMN, NotesHost, Piece, append_notes, fill, fill_into, fill_offset, indent_block,
    is_loose, item_separator, offset_as_i32, ordered_marker, render_html_attr,
};
use crate::markdown_common::{
    attr_is_empty, atx_heading_marker, indent_code, is_html_format, needs_separator,
    offset_horizontal_rule, quote_block,
};

use self::helpers::{
    caption_blocks_as_inlines, checkbox_state, collapse_trailing_newline, colon_fence_len,
    div_opener, escape_leading_markers, extended_code_info, fence_run_len, github_code_info,
    header_attr_implicit, is_tex_format, single_paragraph, strip_checkbox,
};
use self::yaml::yaml_metadata_block;

mod helpers;
mod inlines;
mod tables;
mod yaml;

/// The rendering configuration shared by every entry point and exposed to sibling writers that embed
/// markdown (the outline writer renders note text through this engine). The active [`Extensions`]
/// set decides which constructs have native syntax versus a fallback. `cmark` marks the `CommonMark`
/// writer family (`gfm`, `commonmark_x`) as opposed to the `markdown`-dialect family (`markdown` and
/// the sparse dialects): the two families share nearly identical extension sets but differ in a handful
/// of constructs no extension can distinguish: a div with no fenced-div syntax wraps in raw `<div>`
/// for the former and renders its contents transparently for the latter; an ordered list with no
/// `fancy_lists`/`startnum` keeps its delimiter and start number for the former and collapses to
/// `1.` for the latter; a hard line break writes `\` for the former and two trailing spaces for the
/// latter. The flag also selects a fenced div's braced `{.class}` shorthand over the bare `class`
/// form.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MarkdownConfig {
    extensions: Extensions,
    cmark: bool,
}

impl MarkdownConfig {
    /// The full-featured markdown dialect, used when a sibling writer embeds markdown text.
    #[cfg(feature = "opml")]
    pub(crate) fn extended() -> Self {
        Self {
            extensions: presets::MARKDOWN,
            cmark: false,
        }
    }

    fn has(self, ext: Extension) -> bool {
        self.extensions.contains(ext)
    }

    /// Whether spans (underline, small caps, the generic `Span`) have native bracketed-span syntax
    /// available, as opposed to an HTML `<span>` fallback.
    fn span_syntax(self) -> bool {
        self.has(Extension::BracketedSpans) || self.has(Extension::NativeSpans)
    }

    /// The marker a hard line break is written with: a trailing `\` for the `CommonMark` family and
    /// any `markdown` dialect with `escaped_line_breaks`, two trailing spaces otherwise.
    fn hard_break(self) -> &'static str {
        if self.cmark || self.has(Extension::EscapedLineBreaks) {
            "\\"
        } else {
            "  "
        }
    }
}

/// Render a document with a markdown-family writer. The active extension set is the one the caller
/// selected through the format spec; when the caller supplied none (a direct writer invocation
/// rather than a `convert` that seeds the target's own extensions) the writer's `default` dialect
/// set is used. `cmark` selects the `CommonMark` writer family's behaviors (see [`MarkdownConfig`]).
fn render_dialect(
    document: &Document,
    options: &WriterOptions,
    default: Extensions,
    cmark: bool,
) -> String {
    let extensions = if options.extensions.is_empty() {
        default
    } else {
        options.extensions
    };
    let config = MarkdownConfig { extensions, cmark };
    render_document(
        document,
        config,
        options.columns.unwrap_or(FILL_COLUMN),
        options.wrap,
    )
}

/// Renders a document to the full-featured markdown dialect.
#[derive(Debug, Default, Clone, Copy)]
pub struct MarkdownWriter;

impl Writer for MarkdownWriter {
    fn write(&self, document: &Document, options: &WriterOptions) -> Result<String> {
        Ok(render_dialect(document, options, presets::MARKDOWN, false))
    }

    fn default_template(&self) -> Option<&'static str> {
        Some(include_str!("templates/default.markdown"))
    }

    fn title_block(&self, document: &Document, options: &WriterOptions) -> Result<Option<String>> {
        yaml_metadata_block(self, document, options)
    }

    fn body_ends_with_newline(&self) -> bool {
        true
    }
}

/// Renders a document to the GitHub-flavored markdown dialect.
#[derive(Debug, Default, Clone, Copy)]
pub struct GfmWriter;

impl Writer for GfmWriter {
    fn write(&self, document: &Document, options: &WriterOptions) -> Result<String> {
        Ok(render_dialect(document, options, presets::GFM, true))
    }

    fn default_template(&self) -> Option<&'static str> {
        Some(include_str!("templates/default.gfm"))
    }

    fn title_block(&self, document: &Document, options: &WriterOptions) -> Result<Option<String>> {
        yaml_metadata_block(self, document, options)
    }

    fn body_ends_with_newline(&self) -> bool {
        true
    }

    // No syntax for a link's identifier; contents entries link without a back-reference anchor.
    fn toc_link_anchors(&self) -> bool {
        false
    }
}

/// Renders a document to the `CommonMark` dialect with a broad set of inline and block extensions
/// enabled. Like the full markdown dialect it emits native syntax for spans, sub/superscript,
/// definition lists, and fenced divs, differing chiefly in that a fenced div always carries a braced
/// attribute block rather than the bare single-class shorthand.
#[derive(Debug, Default, Clone, Copy)]
pub struct CommonmarkXWriter;

impl Writer for CommonmarkXWriter {
    fn write(&self, document: &Document, options: &WriterOptions) -> Result<String> {
        Ok(render_dialect(
            document,
            options,
            presets::COMMONMARK_X,
            true,
        ))
    }

    fn default_template(&self) -> Option<&'static str> {
        Some(include_str!("templates/default.markdown"))
    }

    fn title_block(&self, document: &Document, options: &WriterOptions) -> Result<Option<String>> {
        yaml_metadata_block(self, document, options)
    }

    fn body_ends_with_newline(&self) -> bool {
        true
    }
}

/// Renders a document to the legacy GitHub Markdown dialect (`markdown_github`): backtick-fenced
/// code, pipe tables, strikeout, and task lists, with everything outside that set (spans,
/// sub/superscript, definition lists, fenced divs, math) falling back to HTML or indented forms.
#[derive(Debug, Default, Clone, Copy)]
pub struct MarkdownGithubWriter;

impl Writer for MarkdownGithubWriter {
    fn write(&self, document: &Document, options: &WriterOptions) -> Result<String> {
        Ok(render_dialect(
            document,
            options,
            presets::MARKDOWN_GITHUB,
            false,
        ))
    }

    fn default_template(&self) -> Option<&'static str> {
        Some(include_str!("templates/default.markdown"))
    }

    fn body_ends_with_newline(&self) -> bool {
        true
    }

    // No syntax for a link's identifier; contents entries link without a back-reference anchor.
    fn toc_link_anchors(&self) -> bool {
        false
    }
}

/// Renders a document to the PHP Markdown Extra dialect (`markdown_phpextra`): tilde-fenced code,
/// pipe tables, definition lists, footnotes, and header/link attributes, with everything outside
/// that set (strikeout, spans, sub/superscript, math, fenced divs) falling back to HTML or
/// indented forms. Its code fences use tildes since the dialect lacks backtick code blocks.
#[derive(Debug, Default, Clone, Copy)]
pub struct MarkdownPhpextraWriter;

impl Writer for MarkdownPhpextraWriter {
    fn write(&self, document: &Document, options: &WriterOptions) -> Result<String> {
        Ok(render_dialect(
            document,
            options,
            presets::MARKDOWN_PHPEXTRA,
            false,
        ))
    }

    fn default_template(&self) -> Option<&'static str> {
        Some(include_str!("templates/default.markdown"))
    }

    fn body_ends_with_newline(&self) -> bool {
        true
    }
}

/// Renders a document to the `MultiMarkdown` dialect (`markdown_mmd`): backtick-fenced code, pipe
/// tables, definition lists, footnotes, sub/superscript, and dollar math, with everything outside
/// that set (strikeout, spans, header attributes, fenced divs) falling back to HTML or indented
/// forms.
#[derive(Debug, Default, Clone, Copy)]
pub struct MarkdownMmdWriter;

impl Writer for MarkdownMmdWriter {
    fn write(&self, document: &Document, options: &WriterOptions) -> Result<String> {
        Ok(render_dialect(
            document,
            options,
            presets::MARKDOWN_MMD,
            false,
        ))
    }

    fn default_template(&self) -> Option<&'static str> {
        Some(include_str!("templates/default.markdown"))
    }

    fn body_ends_with_newline(&self) -> bool {
        true
    }

    // No syntax for a link's identifier; contents entries link without a back-reference anchor.
    fn toc_link_anchors(&self) -> bool {
        false
    }
}

/// Renders a document to the original Markdown dialect (`markdown_strict`): the sparsest `markdown`
/// dialect, with only raw HTML beyond plain Markdown. Code blocks indent, tables and strikeout and
/// sub/superscript fall back to HTML, task-list checkboxes keep their raw glyphs, and every other
/// richer construct degrades to its plainest form.
#[derive(Debug, Default, Clone, Copy)]
pub struct MarkdownStrictWriter;

impl Writer for MarkdownStrictWriter {
    fn write(&self, document: &Document, options: &WriterOptions) -> Result<String> {
        Ok(render_dialect(
            document,
            options,
            presets::MARKDOWN_STRICT,
            false,
        ))
    }

    fn default_template(&self) -> Option<&'static str> {
        Some(include_str!("templates/default.markdown"))
    }

    fn body_ends_with_newline(&self) -> bool {
        true
    }

    // No syntax for a link's identifier; contents entries link without a back-reference anchor.
    fn toc_link_anchors(&self) -> bool {
        false
    }
}

fn render_document(
    document: &Document,
    config: MarkdownConfig,
    width: usize,
    wrap: WrapMode,
) -> String {
    let mut state = State::new(config, width, wrap);
    let body = state.blocks_to_string(&document.blocks, width);
    append_notes(body, &state.footnotes)
}

/// Render a block sequence as a markdown fragment, accumulating footnotes for a trailing section.
/// Exposed so a writer embedding markdown text can render a block list through this engine.
#[cfg(feature = "opml")]
pub(crate) fn render_blocks(
    blocks: &[Block],
    config: MarkdownConfig,
    width: usize,
    wrap: WrapMode,
) -> String {
    let mut state = State::new(config, width, wrap);
    let body = state.blocks_to_string(blocks, width);
    append_notes(body, &state.footnotes)
}

#[derive(Debug)]
struct State {
    config: MarkdownConfig,
    wrap: WrapMode,
    width: usize,
    footnotes: Vec<String>,
    /// Whether rendering is inside an HTML `<a>` fallback's label, where a nested link must
    /// downgrade to a `<span>` (HTML forbids an anchor inside an anchor).
    in_anchor: bool,
    /// How many tables the current render is nested inside, counting the one being rendered.
    table_depth: usize,
}

impl State {
    fn new(config: MarkdownConfig, width: usize, wrap: WrapMode) -> Self {
        Self {
            config,
            wrap,
            width,
            footnotes: Vec::new(),
            in_anchor: false,
            table_depth: 0,
        }
    }

    fn blocks_to_string(&mut self, blocks: &[Block], width: usize) -> String {
        let mut out = String::new();
        let mut previous: Option<&Block> = None;
        for block in blocks {
            let text = self.block(block, width);
            if text.is_empty() {
                continue;
            }
            if let Some(previous) = previous {
                if needs_separator(previous, block) {
                    out.push_str("\n\n<!-- -->\n\n");
                } else if matches!(previous, Block::Plain(_)) {
                    out.push('\n');
                } else {
                    out.push_str("\n\n");
                }
            }
            out.push_str(&text);
            previous = Some(block);
        }
        out
    }

    fn block(&mut self, block: &Block, width: usize) -> String {
        match block {
            Block::Plain(inlines) | Block::Para(inlines) => {
                let mut pieces = self.pieces(inlines);
                escape_leading_markers(&mut pieces);
                fill(&pieces, width, self.wrap)
            }
            Block::Header(level, attr, inlines) => self.header(*level, attr, inlines),
            Block::CodeBlock(attr, text) => self.code_block(attr, text),
            Block::RawBlock(format, text) => self.raw_block(format, text),
            Block::BlockQuote(blocks) => {
                let body = self.blocks_to_string(blocks, width.saturating_sub(2));
                quote_block(&body)
            }
            Block::BulletList(items) => self.bullet_list(items, width),
            Block::OrderedList(attrs, items) => self.ordered_list(attrs, items, width),
            Block::DefinitionList(items) => self.definition_list(items, width),
            Block::HorizontalRule => "-".repeat(width),
            Block::Div(attr, blocks) => self.div(attr, blocks, width),
            Block::LineBlock(lines) => self.line_block(lines),
            Block::Figure(attr, caption, blocks) => self.figure(attr, caption, blocks),
            Block::Table(table) => self.table(table, width),
        }
    }

    fn header(&mut self, level: i32, attr: &Attr, inlines: &[Inline]) -> String {
        let hashes = atx_heading_marker(level);
        let text = self.inlines_oneline(inlines);
        let auto_identifiers = self.config.has(Extension::AutoIdentifiers)
            || self.config.has(Extension::GfmAutoIdentifiers);
        let implicit = header_attr_implicit(attr, inlines, auto_identifiers);
        let suffix = if self.config.has(Extension::MmdHeaderIdentifiers) {
            // MultiMarkdown writes only a trailing `[id]`; classes/key-values drop, auto-regenerable ids are omitted.
            if implicit {
                String::new()
            } else {
                format!(" [{}]", attr.id)
            }
        } else if !self.config.has(Extension::HeaderAttributes) || implicit {
            String::new()
        } else {
            format!(" {}", attr_braces(attr))
        };
        if text.is_empty() {
            format!("{hashes}{suffix}").trim_end().to_owned()
        } else {
            format!("{hashes} {text}{suffix}")
        }
    }

    fn code_block(&mut self, attr: &Attr, text: &str) -> String {
        let body = text.strip_suffix('\n').unwrap_or(text);
        let backtick = self.config.has(Extension::BacktickCodeBlocks);
        let fenced = backtick || self.config.has(Extension::FencedCodeBlocks);
        let info = if fenced {
            if self.config.has(Extension::FencedCodeAttributes) {
                extended_code_info(attr)
            } else {
                github_code_info(attr)
            }
        } else {
            None
        };
        let Some(info) = info else {
            return indent_code(text);
        };
        let fence_char = if backtick { '`' } else { '~' };
        let fence = fence_char
            .to_string()
            .repeat(fence_run_len(body, fence_char));
        if body.is_empty() {
            format!("{fence}{info}\n{fence}")
        } else {
            format!("{fence}{info}\n{body}\n{fence}")
        }
    }

    fn raw_block(&mut self, format: &Format, text: &str) -> String {
        // Verbatim only for a natively embeddable format (HTML under `raw_html`, TeX under `raw_tex`);
        // otherwise the ```` ```{=fmt} ```` fence needs `raw_attribute`, else the block is dropped.
        let native = if is_html_format(format) {
            self.config.has(Extension::RawHtml)
        } else if is_tex_format(format) {
            self.config.has(Extension::RawTex)
        } else {
            false
        };
        if native {
            return collapse_trailing_newline(text);
        }
        if !self.config.has(Extension::RawAttribute) {
            return String::new();
        }
        let body = collapse_trailing_newline(text);
        let fence = "`".repeat(fence_run_len(&body, '`'));
        if body.is_empty() {
            format!("{fence}{{={}}}\n{fence}", format.0)
        } else {
            format!("{fence}{{={}}}\n{body}\n{fence}", format.0)
        }
    }

    fn div(&mut self, attr: &Attr, blocks: &[Block], width: usize) -> String {
        let body = self.blocks_to_string(blocks, width);
        if !self.config.has(Extension::FencedDivs) {
            // CommonMark family and HTML-parsing `markdown` dialects wrap in a literal `<div>` (with
            // `data-markdown="1"` under `markdown_attribute`); sparse dialects render contents transparently.
            let marker = self.config.has(Extension::MarkdownAttribute);
            if self.config.cmark
                || self.config.has(Extension::NativeDivs)
                || self.config.has(Extension::MarkdownInHtmlBlocks)
                || marker
            {
                let data = if marker { " data-markdown=\"1\"" } else { "" };
                return format!("<div{}{data}>\n\n{body}\n\n</div>", render_html_attr(attr));
            }
            return body;
        }
        let fence = ":".repeat(colon_fence_len(&body));
        let opener = div_opener(attr, self.config.cmark);
        if body.is_empty() {
            format!("{fence}{opener}\n{fence}")
        } else {
            format!("{fence}{opener}\n{body}\n{fence}")
        }
    }

    fn line_block(&mut self, lines: &[Vec<Inline>]) -> String {
        if !self.config.has(Extension::LineBlocks) {
            let rendered: Vec<String> = lines
                .iter()
                .map(|line| self.inlines_oneline(line))
                .collect();
            return rendered.join(&format!("{}\n", self.config.hard_break()));
        }
        let rendered: Vec<String> = lines
            .iter()
            .map(|line| format!("| {}", self.inlines_oneline(line)))
            .collect();
        rendered.join("\n")
    }

    fn bullet_list(&mut self, items: &[Vec<Block>], width: usize) -> String {
        if self.config.has(Extension::TaskLists)
            && let Some(rendered) = self.task_list(items, width)
        {
            return rendered;
        }
        let loose = is_loose(items);
        let body_width = width.saturating_sub(2);
        let rendered: Vec<String> = items
            .iter()
            .map(|item| self.list_item(item, body_width, "- ", "  "))
            .collect();
        rendered.join(item_separator(loose))
    }

    /// Render one list item's body under its marker. A single-paragraph item is filled straight into
    /// the item buffer with the marker prefixes; any other item keeps the render-then-indent path.
    fn list_item(&mut self, item: &[Block], body_width: usize, first: &str, rest: &str) -> String {
        if let Some(inlines) = single_paragraph(item) {
            let mut pieces = self.pieces(inlines);
            escape_leading_markers(&mut pieces);
            let mut out = String::new();
            fill_into(&mut out, &pieces, body_width, self.wrap, first, rest);
            return out;
        }
        let body = self.blocks_to_string(item, body_width);
        let body = offset_horizontal_rule(item, body);
        indent_block(&body, first, rest)
    }

    /// A bullet list every item of which begins with a checkbox glyph renders as a task list. Each
    /// item's leading glyph and the space after it are replaced with the `[ ]`/`[x]` marker.
    fn task_list(&mut self, items: &[Vec<Block>], width: usize) -> Option<String> {
        let marks: Option<Vec<bool>> = items.iter().map(|item| checkbox_state(item)).collect();
        let marks = marks?;
        let loose = is_loose(items);
        let body_width = width.saturating_sub(2);
        let rendered: Vec<String> = items
            .iter()
            .zip(marks)
            .map(|(item, checked)| {
                let stripped = strip_checkbox(item);
                let body = self.blocks_to_string(&stripped, body_width);
                let marker = if checked { "- [x] " } else { "- [ ] " };
                indent_block(&body, marker, "  ")
            })
            .collect();
        Some(rendered.join(item_separator(loose)))
    }

    fn ordered_list(
        &mut self,
        attrs: &ListAttributes,
        items: &[Vec<Block>],
        width: usize,
    ) -> String {
        let loose = is_loose(items);
        let (style, delim) = self.ordered_marks(attrs);
        let start = self.ordered_start(attrs);
        let rendered: Vec<String> = items
            .iter()
            .enumerate()
            .map(|(offset, item)| {
                let number = start.saturating_add(offset_as_i32(offset));
                let marker = ordered_marker(number, style, delim);
                let field = (marker.chars().count() + 1).max(4);
                let first = format!("{marker:<field$}");
                let rest = " ".repeat(field);
                self.list_item(item, width.saturating_sub(field), &first, &rest)
            })
            .collect();
        rendered.join(item_separator(loose))
    }

    /// The numeral style and delimiter to render an ordered list with. With the fancy-list extension
    /// the source style and delimiter are kept. Without it the `CommonMark` family still collapses the
    /// style to decimal but keeps a closing-parenthesis delimiter; the `markdown` dialects have
    /// no rich-list syntax at all and collapse every list to a decimal period (`1.`).
    fn ordered_marks(&self, attrs: &ListAttributes) -> (ListNumberStyle, ListNumberDelim) {
        if self.config.has(Extension::FancyLists) {
            return (attrs.style, attrs.delim);
        }
        if self.config.cmark {
            let delim = match attrs.delim {
                ListNumberDelim::OneParen | ListNumberDelim::TwoParens => ListNumberDelim::OneParen,
                ListNumberDelim::Period | ListNumberDelim::DefaultDelim => ListNumberDelim::Period,
            };
            return (ListNumberStyle::Decimal, delim);
        }
        (ListNumberStyle::Decimal, ListNumberDelim::Period)
    }

    /// The first ordered-list number. The `CommonMark` family and the `startnum` extension honor the
    /// source list's start number; the other `markdown` dialects renumber from 1.
    fn ordered_start(&self, attrs: &ListAttributes) -> i32 {
        if self.config.cmark || self.config.has(Extension::Startnum) {
            attrs.start
        } else {
            1
        }
    }

    fn definition_list(
        &mut self,
        items: &[(Vec<Inline>, Vec<Vec<Block>>)],
        width: usize,
    ) -> String {
        if !self.config.has(Extension::DefinitionLists) {
            return self.definition_list_fallback(items, width);
        }
        let groups: Vec<String> = items
            .iter()
            .map(|(term, definitions)| self.definition_group(term, definitions, width))
            .collect();
        groups.join("\n\n")
    }

    /// One term and its definitions in definition-list syntax: the term on its own line, then each
    /// definition introduced by `:` with a two-column hanging indent.
    fn definition_group(
        &mut self,
        term: &[Inline],
        definitions: &[Vec<Block>],
        width: usize,
    ) -> String {
        let term_line = self.inlines_oneline(term);
        let loose = definitions.iter().any(|blocks| {
            blocks.iter().any(|block| !matches!(block, Block::Plain(_))) || blocks.len() > 1
        });
        let bodies: Vec<String> = definitions
            .iter()
            .map(|definition| {
                let body = self.blocks_to_string(definition, width.saturating_sub(2));
                indent_block(&body, ": ", "  ")
            })
            .collect();
        let separator = if loose { "\n\n" } else { "\n" };
        let definitions = bodies.join(separator);
        if loose {
            format!("{term_line}\n\n{definitions}")
        } else {
            format!("{term_line}\n{definitions}")
        }
    }

    /// Without the definition-list extension there is no native syntax: each term renders as a line
    /// ending in a hard break and its definitions follow as ordinary blocks.
    fn definition_list_fallback(
        &mut self,
        items: &[(Vec<Inline>, Vec<Vec<Block>>)],
        width: usize,
    ) -> String {
        let groups: Vec<String> = items
            .iter()
            .map(|(term, definitions)| {
                let term_line = self.inlines_oneline(term);
                let bodies: Vec<String> = definitions
                    .iter()
                    .map(|definition| self.blocks_to_string(definition, width))
                    .collect();
                let body = bodies.join("\n\n");
                if body.is_empty() {
                    format!("{term_line}  ")
                } else {
                    format!("{term_line}  \n{body}")
                }
            })
            .collect();
        groups.join("\n\n")
    }

    fn figure(&mut self, attr: &Attr, caption: &Caption, blocks: &[Block]) -> String {
        if !self.config.has(Extension::ImplicitFigures) {
            return crate::html::render_fragment(
                &[Block::Figure(
                    Box::new(attr.clone()),
                    Box::new(caption.clone()),
                    blocks.to_vec(),
                )],
                self.wrap,
            );
        }
        if let Some(rendered) = self.implicit_figure(attr, caption, blocks) {
            return rendered;
        }
        crate::html::render_fragment(
            &[Block::Figure(
                Box::new(attr.clone()),
                Box::new(caption.clone()),
                blocks.to_vec(),
            )],
            self.wrap,
        )
    }

    /// A figure renders as a bare image when it carries no attributes and its body is a single image
    /// in a `Plain` block whose own attributes hold neither an identifier nor classes. The visible
    /// text is the caption; the image's alt text, when non-empty and not already equal to the
    /// caption, is preserved as an `alt` attribute. Returns `None` when the shorthand does not apply,
    /// so the caller can fall back to an HTML figure.
    fn implicit_figure(
        &mut self,
        attr: &Attr,
        caption: &Caption,
        blocks: &[Block],
    ) -> Option<String> {
        if !attr_is_empty(attr) {
            return None;
        }
        let [Block::Plain(inlines)] = blocks else {
            return None;
        };
        let [Inline::Image(image_attr, alt, target)] = inlines.as_slice() else {
            return None;
        };
        if !image_attr.id.is_empty() || !image_attr.classes.is_empty() {
            return None;
        }
        let caption_inlines = caption_blocks_as_inlines(&caption.long)?;
        let alt_text = carta_ast::to_plain_text(alt);
        let mut image_attr = image_attr.clone();
        if !alt_text.is_empty() && alt_text != carta_ast::to_plain_text(&caption_inlines) {
            image_attr
                .attributes
                .insert(0, ("alt".into(), alt_text.into()));
        }
        // An image falling back to `<img>` cannot ride the shorthand; the caller emits an HTML `<figure>`.
        if self.image_renders_as_html(&image_attr) {
            return None;
        }
        let mut out = Vec::new();
        self.image(&image_attr, &caption_inlines, target, &mut out);
        Some(pieces_to_string(&out))
    }
}

impl NotesHost for State {
    fn notes(&mut self) -> &mut Vec<String> {
        &mut self.footnotes
    }

    fn render_block(&mut self, block: &Block, width: usize) -> String {
        self.block(block, width)
    }

    fn render_offset_paragraph(
        &mut self,
        inlines: &[Inline],
        width: usize,
        initial: usize,
    ) -> String {
        let pieces = self.pieces(inlines);
        fill_offset(&pieces, width, initial, self.wrap)
    }

    fn base_width(&self) -> usize {
        self.width
    }

    fn record_note(&mut self, blocks: &[Block]) -> String {
        // Without `footnotes` there is no `[^n]` syntax; notes degrade to the numbered `[n]` form.
        if !self.config.has(Extension::Footnotes) {
            return self.numbered_note(blocks);
        }
        let index = self.notes().len();
        self.notes().push(String::new());
        let marker = format!("[^{}]", index + 1);
        let label = format!("{marker}:");
        let body = self.note_body(blocks, 4);
        let starts_inline = matches!(blocks.first(), Some(Block::Plain(_) | Block::Para(_)));
        let rendered = if body.is_empty() {
            label
        } else if starts_inline {
            format!("{label} {body}")
        } else {
            format!("{label}\n{}", indent_block(&body, "    ", "    "))
        };
        if let Some(slot) = self.notes().get_mut(index) {
            *slot = rendered;
        }
        marker
    }

    fn note_body(&mut self, blocks: &[Block], _initial: usize) -> String {
        let width = self.base_width();
        let rendered: Vec<(bool, String)> = blocks
            .iter()
            .enumerate()
            .map(|(position, block)| {
                let is_plain = matches!(block, Block::Plain(_));
                let text = self.render_block(block, width);
                let text = if position == 0 {
                    text
                } else {
                    indent_block(&text, "    ", "    ")
                };
                (is_plain, text)
            })
            .collect();
        crate::common::join_loose(rendered)
    }
}

fn pieces_to_string(pieces: &[Piece]) -> String {
    let mut out = String::new();
    for piece in pieces {
        match piece {
            Piece::Text(text) => out.push_str(text),
            Piece::Space | Piece::Soft => out.push(' '),
            Piece::Hard => out.push('\n'),
        }
    }
    out
}

/// The attribute block of a header, link, image, or code block: `{#id .class key="val"}` with the
/// leading brace.
fn attr_braces(attr: &Attr) -> String {
    format!("{{{}}}", attr_body(attr))
}

/// The body of an attribute block (without the braces): id, then classes, then key/value pairs,
/// each separated by a space. Unlike HTML attributes, unknown keys are emitted verbatim.
fn attr_body(attr: &Attr) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !attr.id.is_empty() {
        parts.push(format!("#{}", attr.id));
    }
    for class in &attr.classes {
        if class.is_empty() {
            continue;
        }
        parts.push(format!(".{class}"));
    }
    for (key, value) in &attr.attributes {
        parts.push(format!("{key}=\"{}\"", value.replace('"', "\\\"")));
    }
    parts.join(" ")
}

#[cfg(test)]
mod escaping_tests;

#[cfg(test)]
mod tests;
