//! Markdown writer engine: renders the document model to a markdown family text format.
//!
//! Every markdown-family dialect shares this engine, parameterized by the `MarkdownConfig` it runs
//! with — chiefly the active [`Extensions`] set. Each construct consults the set to choose its
//! surface: an attribute block on a header, link, image, or span versus a bare or HTML rendering;
//! native subscript/superscript/strikeout versus an HTML tag; fenced versus indented code; fenced
//! divs versus `<div>`; space-aligned, bordered, pipe, or HTML tables; citation syntax versus the
//! display text; dollar, GitHub, or linearized math; raw passthrough with a format tag versus a
//! verbatim or dropped fallback. Smart punctuation is rewritten to ASCII when the `smart` extension
//! is active and emitted as the literal glyph otherwise. Inline content wraps at a fill column of
//! 72. Output carries no trailing newline; the caller appends one.

use std::borrow::Cow;
use std::fmt::Write;

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, Citation, CitationMode, ColWidth, Document, Format,
    Inline, ListAttributes, ListNumberDelim, ListNumberStyle, MathType, MetaValue, Row, Table,
    Target, Text,
};
use carta_core::{Extension, Extensions, Result, WrapMode, Writer, WriterOptions, presets};

use crate::common::{
    FILL_COLUMN, MEASURE_WIDTH, NotesHost, Piece, TableForm, append_notes, block_inlines,
    body_rows, cell_inlines, dash_rule, display_width, escape_attr, extend_multiline_body, fill,
    fill_offset, filled_cells, indent_block, indent_lines, is_known_scheme, is_loose,
    is_percent_escaped_uri, is_simple_cell, item_separator, lay_row, measure_pieces, offset_as_i32,
    ordered_marker, pad_align, pieces_nonempty, quote_marks, render_html_attr, table_form,
};
use crate::grid;

/// The rendering configuration shared by every entry point and exposed to sibling writers that embed
/// markdown (the outline writer renders note text through this engine). The active [`Extensions`]
/// set decides which constructs have native syntax versus a fallback. `cmark` marks the `CommonMark`
/// writer family (`gfm`, `commonmark_x`) as opposed to the `markdown`-dialect family (`markdown` and
/// the sparse dialects): the two families share nearly identical extension sets but differ in a handful
/// of constructs no extension can distinguish — a div with no fenced-div syntax wraps in raw `<div>`
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

    /// Whether the active extension set contains `ext`.
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
/// selected through the format spec; when the caller supplied none — a direct writer invocation
/// rather than a `convert` that seeds the target's own extensions — the writer's `default` dialect
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

    // This dialect has no syntax for a link's identifier, so a contents entry carrying one would
    // degrade to raw HTML; entries link without a back-reference anchor instead.
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
/// code, pipe tables, strikeout, and task lists, with everything outside that set — spans,
/// sub/superscript, definition lists, fenced divs, math — falling back to HTML or indented forms.
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

    // This dialect has no syntax for a link's identifier, so a contents entry carrying one would
    // degrade to raw HTML; entries link without a back-reference anchor instead.
    fn toc_link_anchors(&self) -> bool {
        false
    }
}

/// Renders a document to the PHP Markdown Extra dialect (`markdown_phpextra`): tilde-fenced code,
/// pipe tables, definition lists, footnotes, and header/link attributes, with everything outside
/// that set — strikeout, spans, sub/superscript, math, fenced divs — falling back to HTML or
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
/// that set — strikeout, spans, header attributes, fenced divs — falling back to HTML or indented
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

    // This dialect has no syntax for a link's identifier, so a contents entry carrying one would
    // degrade to raw HTML; entries link without a back-reference anchor instead.
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

    // This dialect has no syntax for a link's identifier, so a contents entry carrying one would
    // degrade to raw HTML; entries link without a back-reference anchor instead.
    fn toc_link_anchors(&self) -> bool {
        false
    }
}

/// Serialize document metadata as a sorted-key YAML block delimited by `---` lines, or `None` when
/// there is no metadata. Scalars render through `writer` so inline markup survives; block values
/// become literal block scalars; sequences and maps nest by indentation. Keys emit in the map's
/// sorted order.
fn yaml_metadata_block(
    writer: &dyn Writer,
    document: &Document,
    options: &WriterOptions,
) -> Result<Option<String>> {
    if document.meta.is_empty() {
        return Ok(None);
    }
    let mut out = String::from("---\n");
    for (key, value) in &document.meta {
        yaml_field(&mut out, writer, key, value, 0, options)?;
    }
    out.push_str("---");
    Ok(Some(out))
}

/// Emit one `key: value` field (and any nested lines) at `depth` levels of two-space indentation.
fn yaml_field(
    out: &mut String,
    writer: &dyn Writer,
    key: &str,
    value: &MetaValue,
    depth: usize,
    options: &WriterOptions,
) -> Result<()> {
    let pad = "  ".repeat(depth);
    match value {
        MetaValue::MetaBool(flag) => {
            let _ = writeln!(out, "{pad}{key}: {flag}");
        }
        MetaValue::MetaString(text) => {
            let scalar = writer.render_meta_inlines(&[Inline::Str(text.clone())], options)?;
            push_yaml_scalar(out, &pad, key, &scalar);
        }
        MetaValue::MetaInlines(inlines) => {
            let scalar = writer.render_meta_inlines(inlines, options)?;
            push_yaml_scalar(out, &pad, key, &scalar);
        }
        MetaValue::MetaBlocks(blocks) => {
            let rendered = writer.render_meta_blocks(blocks, options)?;
            let _ = writeln!(out, "{pad}{key}: |");
            push_yaml_literal(out, &pad, &rendered);
        }
        MetaValue::MetaList(items) => {
            let _ = writeln!(out, "{pad}{key}:");
            for item in items {
                yaml_seq_item(out, writer, item, depth, options)?;
            }
        }
        MetaValue::MetaMap(map) => {
            let _ = writeln!(out, "{pad}{key}:");
            for (sub_key, sub_value) in map {
                yaml_field(out, writer, sub_key, sub_value, depth + 1, options)?;
            }
        }
    }
    Ok(())
}

/// Emit one `- value` sequence entry at `depth` levels of indentation. Scalars sit on the dash line;
/// richer values open a nested block under it.
fn yaml_seq_item(
    out: &mut String,
    writer: &dyn Writer,
    value: &MetaValue,
    depth: usize,
    options: &WriterOptions,
) -> Result<()> {
    let pad = "  ".repeat(depth);
    match value {
        MetaValue::MetaBool(flag) => {
            let _ = writeln!(out, "{pad}- {flag}");
        }
        MetaValue::MetaString(text) => {
            let scalar = writer.render_meta_inlines(&[Inline::Str(text.clone())], options)?;
            push_yaml_scalar_item(out, &pad, &scalar);
        }
        MetaValue::MetaInlines(inlines) => {
            let scalar = writer.render_meta_inlines(inlines, options)?;
            push_yaml_scalar_item(out, &pad, &scalar);
        }
        MetaValue::MetaBlocks(blocks) => {
            let rendered = writer.render_meta_blocks(blocks, options)?;
            let _ = writeln!(out, "{pad}- |");
            push_yaml_literal(out, &format!("{pad}  "), &rendered);
        }
        MetaValue::MetaList(items) => {
            let _ = writeln!(out, "{pad}-");
            for item in items {
                yaml_seq_item(out, writer, item, depth + 1, options)?;
            }
        }
        MetaValue::MetaMap(map) => {
            let _ = writeln!(out, "{pad}-");
            for (sub_key, sub_value) in map {
                yaml_field(out, writer, sub_key, sub_value, depth + 1, options)?;
            }
        }
    }
    Ok(())
}

/// Emit a scalar field, choosing a literal block scalar when the value spans multiple lines.
fn push_yaml_scalar(out: &mut String, pad: &str, key: &str, scalar: &str) {
    if scalar.contains('\n') {
        let _ = writeln!(out, "{pad}{key}: |");
        push_yaml_literal(out, pad, scalar);
    } else {
        let _ = writeln!(out, "{pad}{key}: {}", yaml_inline_scalar(scalar));
    }
}

/// Emit a scalar sequence entry, choosing a literal block scalar when the value spans multiple lines.
fn push_yaml_scalar_item(out: &mut String, pad: &str, scalar: &str) {
    if scalar.contains('\n') {
        let _ = writeln!(out, "{pad}- |");
        push_yaml_literal(out, &format!("{pad}  "), scalar);
    } else {
        let _ = writeln!(out, "{pad}- {}", yaml_inline_scalar(scalar));
    }
}

/// Render a single-line scalar as YAML flow text: a double-quoted string when a plain scalar would be
/// reparsed as something other than its text, otherwise the text verbatim.
fn yaml_inline_scalar(scalar: &str) -> Cow<'_, str> {
    if yaml_needs_quoting(scalar) {
        Cow::Owned(yaml_quote(scalar))
    } else {
        Cow::Borrowed(scalar)
    }
}

/// Whether a single-line scalar must be double-quoted to round-trip as itself. An empty string,
/// surrounding spaces, an embedded colon or ` #` comment opener, a leading YAML indicator character,
/// or a word YAML reads as a boolean or null all force quoting.
fn yaml_needs_quoting(scalar: &str) -> bool {
    if scalar.is_empty() || scalar.starts_with(' ') || scalar.ends_with(' ') {
        return true;
    }
    if scalar.contains(':') || scalar.contains(" #") {
        return true;
    }
    if scalar
        .chars()
        .next()
        .is_some_and(|first| "-?,[]{}#&*!|>'\"%@`".contains(first))
    {
        return true;
    }
    matches!(
        scalar.to_ascii_lowercase().as_str(),
        "true" | "false" | "yes" | "no" | "on" | "off" | "null"
    )
}

/// Double-quote a scalar, escaping backslashes and quotes so it parses back to the same text.
fn yaml_quote(scalar: &str) -> String {
    let mut quoted = String::with_capacity(scalar.len() + 2);
    quoted.push('"');
    for ch in scalar.chars() {
        match ch {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            _ => quoted.push(ch),
        }
    }
    quoted.push('"');
    quoted
}

/// Indent every line of `text` two spaces past `pad`, forming the body of a literal block scalar.
fn push_yaml_literal(out: &mut String, pad: &str, text: &str) {
    for line in text.lines() {
        if line.is_empty() {
            out.push('\n');
        } else {
            let _ = writeln!(out, "{pad}  {line}");
        }
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
}

impl State {
    fn new(config: MarkdownConfig, width: usize, wrap: WrapMode) -> Self {
        Self {
            config,
            wrap,
            width,
            footnotes: Vec::new(),
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
        let hashes = "#".repeat(usize::try_from(level.max(1)).unwrap_or(1));
        let text = self.inlines_oneline(inlines);
        let auto_identifiers = self.config.has(Extension::AutoIdentifiers)
            || self.config.has(Extension::GfmAutoIdentifiers);
        let implicit = header_attr_implicit(attr, inlines, auto_identifiers);
        let suffix = if self.config.has(Extension::MmdHeaderIdentifiers) {
            // MultiMarkdown writes only the identifier, in a trailing `[id]`; classes and key/value
            // pairs are dropped. An attribute that an auto-identifier would regenerate is omitted.
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
        // A raw block round-trips verbatim only when its format is one the dialect can embed
        // natively: HTML under `raw_html`, or TeX under `raw_tex`. Otherwise it needs the
        // `raw_attribute` fenced form (```` ```{=fmt} ````); without that extension it is dropped.
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
            // The `CommonMark` family, and any `markdown` dialect that parses raw HTML divs,
            // wrap the contents in a literal `<div>`; the sparse `markdown` dialects have no
            // div syntax at all and render the contents transparently. The `markdown_attribute`
            // dialects also wrap, tagging the `<div>` with `data-markdown="1"` so its contents are
            // still parsed as Markdown.
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
            .map(|item| {
                let body = self.blocks_to_string(item, body_width);
                let body = offset_horizontal_rule(item, body);
                indent_block(&body, "- ", "  ")
            })
            .collect();
        rendered.join(item_separator(loose))
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
                let body = self.blocks_to_string(item, width.saturating_sub(field));
                let body = offset_horizontal_rule(item, body);
                let first = format!("{marker:<field$}");
                let rest = " ".repeat(field);
                indent_block(&body, &first, &rest)
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
                .insert(0, ("alt".to_owned(), alt_text));
        }
        // If the image itself would fall back to an HTML `<img>`, the shorthand cannot carry it; the
        // caller renders the whole figure as an HTML `<figure>` instead.
        if self.image_renders_as_html(&image_attr) {
            return None;
        }
        let mut out = Vec::new();
        self.image(&image_attr, &caption_inlines, target, &mut out);
        Some(pieces_to_string(&out))
    }

    fn table(&mut self, table: &Table, width: usize) -> String {
        let native = self.config.has(Extension::SimpleTables)
            || self.config.has(Extension::MultilineTables)
            || self.config.has(Extension::GridTables);
        if !native {
            if self.config.has(Extension::PipeTables) {
                return self.github_table(table, width);
            }
            return self.html_table(table);
        }
        if table.col_specs.is_empty() {
            return String::new();
        }
        let form = table_form(table);
        let body = match form {
            TableForm::Simple => self.simple_table(table),
            TableForm::Multiline => self.multiline_table(table, width),
            TableForm::Grid => self.grid_table(table, width),
        };
        match self.table_caption(table, form, width) {
            Some(caption) if body.is_empty() => caption,
            Some(caption) => format!("{body}\n\n{caption}"),
            None => body,
        }
    }

    /// Render a table as a raw HTML block, for a dialect whose extension set gives it no native
    /// table syntax. A raw HTML block in markdown is terminated by a blank line, so any blank line
    /// the table's HTML spans — an empty body, the gap between two row groups — is collapsed by
    /// [`encode_html_block_blank_lines`] so the whole table stays a single block.
    fn html_table(&self, table: &Table) -> String {
        let html =
            crate::html::render_fragment(&[Block::Table(Box::new(table.clone()))], self.wrap);
        encode_html_block_blank_lines(&html)
    }

    /// A GitHub table: a pipe table when every cell is a single line and no cell spans, otherwise an
    /// HTML table. A column-aligned pipe table whose columns together exceed the fill column drops to
    /// a narrow form with single-space cell padding. The caption follows the table as its own block.
    fn github_table(&mut self, table: &Table, width: usize) -> String {
        if !pipe_representable(table) {
            return self.html_table(table);
        }
        let columns = table.col_specs.len();
        if columns == 0 {
            return "||\n||".to_owned();
        }
        let aligns: Vec<Alignment> = table
            .col_specs
            .iter()
            .map(|spec| spec.align.clone())
            .collect();
        let head_rows: Vec<&Row> = table.head.rows.iter().collect();
        let data_rows: Vec<&Row> = body_rows(table)
            .into_iter()
            .chain(table.foot.rows.iter())
            .collect();
        let header = head_rows.first();
        let header_cells = self.pipe_cells(header.map(|row| row.cells.as_slice()), columns);
        let data: Vec<Vec<String>> = data_rows
            .iter()
            .map(|row| self.pipe_cells(Some(&row.cells), columns))
            .collect();
        let mut col_widths = vec![3usize; columns];
        for cells in std::iter::once(&header_cells).chain(data.iter()) {
            for (index, cell) in cells.iter().enumerate() {
                if let Some(slot) = col_widths.get_mut(index) {
                    *slot = (*slot).max(display_width(cell));
                }
            }
        }
        let narrow = col_widths.iter().sum::<usize>() > width;
        let (row_widths, sep_widths) = if narrow {
            (vec![0usize; columns], vec![2usize; columns])
        } else {
            (col_widths.clone(), col_widths)
        };
        let mut lines = vec![pipe_row(&header_cells, &row_widths, &aligns)];
        lines.push(pipe_separator(&sep_widths, &aligns));
        for cells in &data {
            lines.push(pipe_row(cells, &row_widths, &aligns));
        }
        let table_text = lines.join("\n");
        match self.github_caption(&table.caption, &table.attr, width) {
            Some(caption) => format!("{table_text}\n\n{caption}"),
            None => table_text,
        }
    }

    /// The GitHub-table caption: the caption blocks reflowed and concatenated with a hard break
    /// between blocks, carrying any table attributes as a trailing `{#id .class key="value"}`
    /// suffix. `None` when the caption is empty.
    fn github_caption(&mut self, caption: &Caption, attr: &Attr, width: usize) -> Option<String> {
        let mut pieces: Vec<Piece> = Vec::new();
        for block in &caption.long {
            if !pieces.is_empty() {
                pieces.push(Piece::Text(self.config.hard_break().to_owned()));
                pieces.push(Piece::Hard);
            }
            self.extend_pieces(block_inlines(block), &mut pieces);
        }
        if !pieces_nonempty(&pieces) {
            return None;
        }
        if let Some(suffix) = attribute_suffix(attr) {
            pieces.push(Piece::Space);
            pieces.push(Piece::Text(suffix));
        }
        Some(fill(&pieces, width, self.wrap))
    }

    /// Render the cells of one pipe-table row to single-line strings, padding the row out to the
    /// column count with empty cells.
    fn pipe_cells(&mut self, cells: Option<&[Cell]>, columns: usize) -> Vec<String> {
        let mut out = Vec::with_capacity(columns);
        let cells = cells.unwrap_or(&[]);
        for index in 0..columns {
            let text = cells
                .get(index)
                .map(|cell| self.cell_oneline(cell))
                .unwrap_or_default();
            out.push(text);
        }
        out
    }

    /// Render a cell's content to a single line for a pipe table, escaping the cell delimiter.
    fn cell_oneline(&mut self, cell: &Cell) -> String {
        let inlines = cell_inlines(cell);
        self.inlines_oneline(inlines)
            .replace('|', "\\|")
            .trim()
            .to_owned()
    }

    /// A simple table: one line per cell, the column width sized to the widest cell plus two. A
    /// non-empty header is underlined with a per-column dash rule; a headerless table is fenced by
    /// dash rules above and below. Indented two columns.
    fn simple_table(&mut self, table: &Table) -> String {
        let columns = table.col_specs.len();
        let aligns: Vec<&Alignment> = table.col_specs.iter().map(|spec| &spec.align).collect();
        let header: Vec<Vec<String>> = table
            .head
            .rows
            .iter()
            .map(|row| self.simple_row(row, columns))
            .collect();
        let body: Vec<Vec<String>> = body_rows(table)
            .iter()
            .map(|row| self.simple_row(row, columns))
            .collect();
        let has_header = header
            .iter()
            .any(|row| row.iter().any(|text| !text.is_empty()));

        let mut field = vec![0usize; columns];
        for row in header.iter().chain(body.iter()) {
            for (index, text) in row.iter().enumerate() {
                if let Some(width) = field.get_mut(index) {
                    *width = (*width).max(display_width(text) + 2);
                }
            }
        }
        let rule = dash_rule(&field);
        let mut lines: Vec<String> = Vec::new();
        let lay = |row: &[String]| {
            let cells: Vec<Vec<String>> = row.iter().map(|text| vec![text.clone()]).collect();
            lay_row(&cells, &field, &aligns)
        };
        if has_header {
            for row in &header {
                lines.extend(lay(row));
            }
            lines.push(rule);
            for row in &body {
                lines.extend(lay(row));
            }
        } else {
            lines.push(rule.clone());
            for row in &body {
                lines.extend(lay(row));
            }
            lines.push(rule);
        }
        indent_lines(&lines, 2)
    }

    /// Render a row's cells to single lines, one per column, padding a short row with empty cells.
    fn simple_row(&mut self, row: &Row, columns: usize) -> Vec<String> {
        let mut out = vec![String::new(); columns];
        for (index, cell) in row.cells.iter().enumerate() {
            if let Some(slot) = out.get_mut(index) {
                *slot = self.inlines_oneline(cell_inlines(cell));
            }
        }
        out
    }

    /// A multiline table: cells wrap within their column and rows are separated by blank lines.
    /// Column widths come from explicit fractional specs (floored at the widest unbreakable word)
    /// or, lacking those, from the natural content width. Indented two columns.
    fn multiline_table(&mut self, table: &Table, width: usize) -> String {
        let columns = table.col_specs.len();
        let aligns: Vec<&Alignment> = table.col_specs.iter().map(|spec| &spec.align).collect();
        let header: Vec<Vec<Vec<Piece>>> = table
            .head
            .rows
            .iter()
            .map(|row| self.row_pieces(row, columns))
            .collect();
        let body: Vec<Vec<Vec<Piece>>> = body_rows(table)
            .iter()
            .map(|row| self.row_pieces(row, columns))
            .collect();
        let has_header = header
            .iter()
            .any(|row| row.iter().any(|cell| pieces_nonempty(cell)));

        let mut natural = vec![0usize; columns];
        let mut minword = vec![0usize; columns];
        for row in header.iter().chain(body.iter()) {
            for (index, cell) in row.iter().enumerate() {
                let (cell_width, word) = measure_pieces(cell);
                if let Some(value) = natural.get_mut(index) {
                    *value = (*value).max(cell_width);
                }
                if let Some(value) = minword.get_mut(index) {
                    *value = (*value).max(word);
                }
            }
        }
        let field: Vec<usize> = (0..columns)
            .map(
                |index| match table.col_specs.get(index).map(|spec| &spec.width) {
                    Some(ColWidth::ColWidth(fraction)) if *fraction > 0.0 => {
                        // A bounded fraction scaled to a small column width: `floor`/`max(0.0)` make
                        // the conversion exact and non-negative.
                        #[allow(
                            clippy::cast_precision_loss,
                            clippy::cast_possible_truncation,
                            clippy::cast_sign_loss
                        )]
                        let scaled =
                            (fraction * width.saturating_sub(1) as f64).floor().max(0.0) as usize;
                        scaled.max(minword.get(index).copied().unwrap_or(0) + 2)
                    }
                    _ => natural.get(index).copied().unwrap_or(0) + 2,
                },
            )
            .collect();

        let contiguous = "-".repeat(field.iter().sum::<usize>() + columns.saturating_sub(1));
        let percolumn = dash_rule(&field);
        let mut lines: Vec<String> = Vec::new();
        if has_header {
            lines.push(contiguous.clone());
            for row in &header {
                lines.extend(lay_row(&filled_cells(row, &field), &field, &aligns));
            }
            lines.push(percolumn);
            extend_multiline_body(&mut lines, &body, &field, &aligns);
            lines.push(contiguous);
        } else {
            lines.push(percolumn.clone());
            extend_multiline_body(&mut lines, &body, &field, &aligns);
            lines.push(percolumn);
        }
        indent_lines(&lines, 2)
    }

    /// Render a row's cells to inline pieces, one entry per column, padding a short row with empty
    /// cells. Building the pieces once records any footnotes a single time.
    fn row_pieces(&mut self, row: &Row, columns: usize) -> Vec<Vec<Piece>> {
        let mut out: Vec<Vec<Piece>> = (0..columns).map(|_| Vec::new()).collect();
        for (index, cell) in row.cells.iter().enumerate() {
            if let Some(slot) = out.get_mut(index) {
                *slot = self.pieces(cell_inlines(cell));
            }
        }
        out
    }

    /// A bordered grid table built on the shared grid engine, with content widths from explicit
    /// fractional specs when present and a content-proportional fit otherwise.
    fn grid_table(&mut self, table: &Table, width: usize) -> String {
        let columns = table.col_specs.len();
        let aligns: Vec<Alignment> = table
            .col_specs
            .iter()
            .map(|spec| spec.align.clone())
            .collect();
        let head: Vec<&Row> = table.head.rows.iter().collect();
        let body = body_rows(table);
        let foot: Vec<&Row> = table.foot.rows.iter().collect();
        let head_layout = grid::place_columns(&head, columns);
        let body_layout = grid::place_columns(&body, columns);
        let foot_layout = grid::place_columns(&foot, columns);

        let snapshot = self.footnotes.len();
        let mut natural = vec![0usize; columns];
        let mut minword = vec![0usize; columns];
        for (rows, layout) in [
            (&head, &head_layout),
            (&body, &body_layout),
            (&foot, &foot_layout),
        ] {
            self.measure_grid(rows, layout, &mut natural, &mut minword);
        }
        self.footnotes.truncate(snapshot);

        let colspans: Vec<(usize, usize)> = [&head_layout, &body_layout, &foot_layout]
            .into_iter()
            .flatten()
            .flatten()
            .copied()
            .filter(|&(_, span)| span > 1)
            .collect();
        let content = grid::grid_content_widths(
            &table.col_specs,
            &natural,
            &minword,
            &colspans,
            columns,
            width,
            self.wrap,
        );
        let col_widths: Vec<usize> = content
            .iter()
            .map(|content_width| content_width + 2)
            .collect();
        let head_grid = self.grid_rows(&head, &head_layout, &content);
        let body_grid = self.grid_rows(&body, &body_layout, &content);
        let foot_grid = self.grid_rows(&foot, &foot_layout, &content);

        grid::render(&grid::GridTable {
            col_widths,
            aligns: Some(aligns.as_slice()),
            head: head_grid,
            body: body_grid,
            foot: foot_grid,
        })
    }

    fn measure_grid(
        &mut self,
        rows: &[&Row],
        layout: &[Vec<(usize, usize)>],
        natural: &mut [usize],
        minword: &mut [usize],
    ) {
        for (row_index, row) in rows.iter().enumerate() {
            for (cell_index, cell) in row.cells.iter().enumerate() {
                let Some(&(start, span)) = layout
                    .get(row_index)
                    .and_then(|placements| placements.get(cell_index))
                else {
                    continue;
                };
                let lines = self.cell_lines(&cell.content, MEASURE_WIDTH);
                let (width, word) = grid::measure_lines(&lines);
                let share_natural = width.div_ceil(span.max(1));
                let share_word = word.div_ceil(span.max(1));
                for column in start..start + span {
                    if let Some(value) = natural.get_mut(column) {
                        *value = (*value).max(share_natural);
                    }
                    if let Some(value) = minword.get_mut(column) {
                        *value = (*value).max(share_word);
                    }
                }
            }
        }
    }

    fn grid_rows(
        &mut self,
        rows: &[&Row],
        layout: &[Vec<(usize, usize)>],
        content: &[usize],
    ) -> Vec<grid::GridRow> {
        let mut result = Vec::with_capacity(rows.len());
        for (row_index, row) in rows.iter().enumerate() {
            let mut cells = Vec::with_capacity(row.cells.len());
            for (cell_index, cell) in row.cells.iter().enumerate() {
                let Some(&(start, span)) = layout
                    .get(row_index)
                    .and_then(|placements| placements.get(cell_index))
                else {
                    continue;
                };
                let width = grid::merged_width(content, start, span);
                let lines = self.cell_lines(&cell.content, width);
                cells.push(grid::GridCell {
                    lines,
                    row_span: grid::span_count(cell.row_span),
                    col_span: grid::span_count(cell.col_span),
                });
            }
            result.push(grid::GridRow { cells });
        }
        result
    }

    fn cell_lines(&mut self, content: &[Block], width: usize) -> Vec<String> {
        let rendered = self.blocks_to_string(content, width.max(1));
        if rendered.is_empty() {
            Vec::new()
        } else {
            rendered.split('\n').map(str::to_owned).collect()
        }
    }

    /// The caption block, prefixed `: ` and indented to match the table form (two columns for simple
    /// and multiline tables, none for grids). A non-empty caption carries any table attributes as a
    /// trailing `{#id .class key="value"}` suffix.
    fn table_caption(&mut self, table: &Table, form: TableForm, width: usize) -> Option<String> {
        let base = if matches!(form, TableForm::Grid) {
            0
        } else {
            2
        };
        let mut pieces: Vec<Piece> = Vec::new();
        for block in &table.caption.long {
            if !pieces.is_empty() {
                pieces.push(Piece::Text(self.config.hard_break().to_owned()));
                pieces.push(Piece::Hard);
            }
            self.extend_pieces(block_inlines(block), &mut pieces);
        }
        if !pieces_nonempty(&pieces) {
            return None;
        }
        if let Some(suffix) = attribute_suffix(&table.attr) {
            pieces.push(Piece::Space);
            pieces.push(Piece::Text(suffix));
        }
        let body = fill_offset(&pieces, width.saturating_sub(base), 2, self.wrap);
        let first = format!("{}: ", " ".repeat(base));
        let rest = " ".repeat(base);
        Some(indent_block(&body, &first, &rest))
    }

    fn inlines_oneline(&mut self, inlines: &[Inline]) -> String {
        let pieces = self.pieces(inlines);
        let mut out = String::new();
        for piece in &pieces {
            match piece {
                Piece::Text(text) => out.push_str(text),
                Piece::Space | Piece::Soft | Piece::Hard => out.push(' '),
            }
        }
        out
    }

    fn pieces(&mut self, inlines: &[Inline]) -> Vec<Piece> {
        let mut out = Vec::new();
        self.extend_pieces(inlines, &mut out);
        out
    }

    fn extend_pieces(&mut self, inlines: &[Inline], out: &mut Vec<Piece>) {
        for (position, inline) in inlines.iter().enumerate() {
            if let Inline::Str(text) = inline
                && let Some(prefix) = text.strip_suffix('!')
                && matches!(inlines.get(position + 1), Some(Inline::Link(..)))
            {
                out.push(Piece::Text(format!("{}\\!", self.escape_str(prefix))));
                continue;
            }
            self.inline(inline, out);
        }
    }

    fn inline(&mut self, inline: &Inline, out: &mut Vec<Piece>) {
        match inline {
            Inline::Str(text) => out.push(Piece::Text(self.escape_str(text))),
            Inline::Emph(inlines) => self.wrap_markup("*", inlines, out),
            Inline::Strong(inlines) => self.wrap_markup("**", inlines, out),
            Inline::Strikeout(inlines) => {
                if self.config.has(Extension::Strikeout) {
                    self.wrap_markup("~~", inlines, out);
                } else {
                    self.wrap_tag("s", inlines, out);
                }
            }
            Inline::Underline(inlines) => {
                if self.config.span_syntax() {
                    self.wrap_span(&underline_attr(), inlines, out);
                } else {
                    self.wrap_tag("u", inlines, out);
                }
            }
            Inline::Superscript(inlines) => {
                if self.config.has(Extension::Superscript) {
                    self.wrap_markup("^", inlines, out);
                } else {
                    self.wrap_tag("sup", inlines, out);
                }
            }
            Inline::Subscript(inlines) => {
                if self.config.has(Extension::Subscript) {
                    self.wrap_markup("~", inlines, out);
                } else {
                    self.wrap_tag("sub", inlines, out);
                }
            }
            Inline::SmallCaps(inlines) => {
                if self.config.span_syntax() {
                    self.wrap_span(&smallcaps_attr(), inlines, out);
                } else {
                    out.push(Piece::Text("<span class=\"smallcaps\">".to_owned()));
                    self.extend_pieces(inlines, out);
                    out.push(Piece::Text("</span>".to_owned()));
                }
            }
            Inline::Quoted(kind, inlines) => {
                let (open, close) = if self.config.has(Extension::Smart) {
                    ascii_quote_marks(kind)
                } else {
                    quote_marks(kind)
                };
                out.push(Piece::Text(open.to_string()));
                self.extend_pieces(inlines, out);
                out.push(Piece::Text(close.to_string()));
            }
            Inline::Cite(citations, inlines) => self.cite(citations, inlines, out),
            Inline::Code(_, text) => out.push(Piece::Text(code_span(text))),
            Inline::Space => out.push(Piece::Space),
            Inline::SoftBreak => out.push(Piece::Soft),
            Inline::LineBreak => {
                out.push(Piece::Text(self.config.hard_break().to_owned()));
                out.push(Piece::Hard);
            }
            Inline::Math(kind, text) => self.math(kind, text, out),
            Inline::RawInline(format, text) => self.raw_inline(format, text, out),
            Inline::Link(attr, inlines, target) => self.link(attr, inlines, target, out),
            Inline::Image(attr, inlines, target) => self.image(attr, inlines, target, out),
            Inline::Span(attr, inlines) => {
                if attr_is_empty(attr) {
                    self.extend_pieces(inlines, out);
                } else if self.config.span_syntax() {
                    self.wrap_span(attr, inlines, out);
                } else {
                    out.push(Piece::Text(format!("<span{}>", render_html_attr(attr))));
                    self.extend_pieces(inlines, out);
                    out.push(Piece::Text("</span>".to_owned()));
                }
            }
            Inline::Note(blocks) => {
                let marker = self.record_note(blocks);
                out.push(Piece::Text(marker));
            }
        }
    }

    /// Render a math node. The GitHub math surface writes an inline `` $`…`$ `` span and a fenced
    /// ```` ```math ```` display block; the dollar surface writes `$…$`/`$$…$$`; the single- and
    /// double-backslash surfaces write `\(…\)`/`\[…\]` and `\\(…\\)`/`\\[…\\]`. With no math syntax
    /// at all the expression linearizes to inline markup, and a display expression then occupies its
    /// own source line, set off from the surrounding text by line breaks.
    fn math(&mut self, kind: &MathType, text: &str, out: &mut Vec<Piece>) {
        if self.config.has(Extension::TexMathGfm) {
            let rendered = match kind {
                MathType::InlineMath => format!("$`{text}`$"),
                MathType::DisplayMath => format!("``` math\n{text}\n```"),
            };
            out.push(Piece::Text(rendered));
            return;
        }
        if self.config.has(Extension::TexMathDollars) {
            let rendered = match kind {
                MathType::InlineMath => format!("${text}$"),
                MathType::DisplayMath => format!("$${text}$$"),
            };
            out.push(Piece::Text(rendered));
            return;
        }
        if self.config.has(Extension::TexMathSingleBackslash) {
            let rendered = match kind {
                MathType::InlineMath => format!("\\({text}\\)"),
                MathType::DisplayMath => format!("\\[{text}\\]"),
            };
            out.push(Piece::Text(rendered));
            return;
        }
        if self.config.has(Extension::TexMathDoubleBackslash) {
            let rendered = match kind {
                MathType::InlineMath => format!("\\\\({text}\\\\)"),
                MathType::DisplayMath => format!("\\\\[{text}\\\\]"),
            };
            out.push(Piece::Text(rendered));
            return;
        }
        if matches!(kind, MathType::DisplayMath) {
            let mut inner = Vec::new();
            self.math_fallback(kind, text, &mut inner);
            if !inner.is_empty() {
                out.push(Piece::Hard);
                out.append(&mut inner);
                out.push(Piece::Hard);
            }
            return;
        }
        self.math_fallback(kind, text, out);
    }

    /// Render a math node when the dialect has no math syntax: the expression linearized to inline
    /// markup when it converts, nothing when it is empty, otherwise the verbatim source wrapped in
    /// the kind's `$`/`$$` delimiters and routed through the running-text path so its literal text is
    /// escaped. Inline source has its edge whitespace trimmed before wrapping; display source is
    /// wrapped as written.
    fn math_fallback(&mut self, kind: &MathType, tex: &str, out: &mut Vec<Piece>) {
        match crate::math::to_inlines(tex) {
            Some(inlines) => {
                for converted in &inlines {
                    self.inline(converted, out);
                }
            }
            None if tex.trim().is_empty() => {}
            None => {
                let (delim, body) = match kind {
                    MathType::DisplayMath => ("$$", tex),
                    MathType::InlineMath => ("$", tex.trim()),
                };
                let fallback = Inline::Str(format!("{delim}{body}{delim}"));
                self.inline(&fallback, out);
            }
        }
    }

    fn raw_inline(&mut self, format: &Format, text: &str, out: &mut Vec<Piece>) {
        if !self.config.has(Extension::RawAttribute) {
            if is_html_format(format) {
                out.push(Piece::Text(text.to_owned()));
            }
            return;
        }
        let fence = "`".repeat((longest_backtick_run(text) + 1).max(1));
        out.push(Piece::Text(format!(
            "{fence}{text}{fence}{{={}}}",
            format.0
        )));
    }

    /// Render a citation. With the citation extension this reconstructs citation syntax; without it
    /// there is no such syntax, so the citation's display inlines render instead.
    fn cite(&mut self, citations: &[Citation], inlines: &[Inline], out: &mut Vec<Piece>) {
        if !self.config.has(Extension::Citations) {
            self.extend_pieces(inlines, out);
            return;
        }
        let text = self.render_citations(citations);
        out.push(Piece::Text(text));
    }

    fn render_citations(&mut self, citations: &[Citation]) -> String {
        if let [single] = citations
            && single.mode == CitationMode::AuthorInText
        {
            let prefix = self.affix(&single.prefix);
            let suffix = self.affix(&single.suffix);
            let mut out = String::new();
            if !prefix.is_empty() {
                out.push_str(&prefix);
                out.push(' ');
            }
            out.push('@');
            out.push_str(&single.id);
            if !suffix.is_empty() {
                out.push(' ');
                out.push_str(&suffix);
            }
            return out;
        }
        let parts: Vec<String> = citations
            .iter()
            .map(|citation| self.citation_in_brackets(citation))
            .collect();
        format!("[{}]", parts.join("; "))
    }

    fn citation_in_brackets(&mut self, citation: &Citation) -> String {
        let prefix = self.affix(&citation.prefix);
        let suffix = self.affix(&citation.suffix);
        let mut out = String::new();
        if !prefix.is_empty() {
            out.push_str(&prefix);
            out.push(' ');
        }
        if citation.mode == CitationMode::SuppressAuthor {
            out.push('-');
        }
        out.push('@');
        out.push_str(&citation.id);
        if !suffix.is_empty() {
            out.push(' ');
            out.push_str(&suffix);
        }
        out
    }

    fn affix(&mut self, inlines: &[Inline]) -> String {
        self.inlines_oneline(inlines)
    }

    fn wrap_markup(&mut self, marker: &str, inlines: &[Inline], out: &mut Vec<Piece>) {
        out.push(Piece::Text(marker.to_owned()));
        self.extend_pieces(inlines, out);
        out.push(Piece::Text(marker.to_owned()));
    }

    fn wrap_tag(&mut self, tag: &str, inlines: &[Inline], out: &mut Vec<Piece>) {
        out.push(Piece::Text(format!("<{tag}>")));
        self.extend_pieces(inlines, out);
        out.push(Piece::Text(format!("</{tag}>")));
    }

    fn wrap_span(&mut self, attr: &Attr, inlines: &[Inline], out: &mut Vec<Piece>) {
        out.push(Piece::Text("[".to_owned()));
        self.extend_pieces(inlines, out);
        out.push(Piece::Text(format!("]{{{}}}", attr_body(attr))));
    }

    fn link(&mut self, attr: &Attr, inlines: &[Inline], target: &Target, out: &mut Vec<Piece>) {
        if (attr_is_empty(attr) || is_autolink_class(attr))
            && let Some(autolink) = autolink(inlines, target)
        {
            out.push(Piece::Text(autolink));
            return;
        }
        if !self.config.has(Extension::LinkAttributes) && !attr_is_empty(attr) {
            out.push(Piece::Text(format!(
                "<a href=\"{}\"{}{}>",
                escape_attr(&target.url),
                render_html_attr(attr),
                title_attr(&target.title)
            )));
            self.extend_pieces(inlines, out);
            out.push(Piece::Text("</a>".to_owned()));
            return;
        }
        out.push(Piece::Text("[".to_owned()));
        self.extend_pieces(inlines, out);
        let attr_suffix = if attr_is_empty(attr) {
            String::new()
        } else {
            attr_braces(attr)
        };
        out.push(Piece::Text(format!(
            "]({}){attr_suffix}",
            destination(target)
        )));
    }

    /// Whether an image carrying `attr` must fall back to an HTML `<img>`: it has attributes the
    /// dialect cannot express as a native `{…}` suffix because it lacks `link_attributes`.
    fn image_renders_as_html(&self, attr: &Attr) -> bool {
        !self.config.has(Extension::LinkAttributes) && (has_dimension(attr) || !attr_is_empty(attr))
    }

    fn image(&mut self, attr: &Attr, inlines: &[Inline], target: &Target, out: &mut Vec<Piece>) {
        if self.image_renders_as_html(attr) {
            out.push(Piece::Text(image_html(attr, inlines, target)));
            return;
        }
        out.push(Piece::Text("![".to_owned()));
        self.extend_pieces(inlines, out);
        let attr_suffix = if attr_is_empty(attr) {
            String::new()
        } else {
            attr_braces(attr)
        };
        out.push(Piece::Text(format!(
            "]({}){attr_suffix}",
            destination(target)
        )));
    }

    /// Escape the markdown-significant characters of running text. Inline-markup openers (`` ` ``,
    /// `*`, `[`, `]`, `<`, `>`), the math delimiter `$`, and entity-introducing `&` are always
    /// escaped; `|` only when pipe tables make it a cell separator; `~` and `^` only when subscript
    /// and superscript have native syntax; and a word-initial `@` only when citations do. A `#` run
    /// that would open a heading is escaped at the start of a line. An `_` is escaped at a word
    /// boundary, and everywhere in a `markdown` dialect without `intraword_underscores` (the
    /// `CommonMark` family never treats an intra-word `_` as emphasis, so it is left literal there).
    /// A backslash is escaped per the raw-TeX extension. Smart-punctuation glyphs are rewritten to
    /// ASCII when the `smart` extension is active.
    fn escape_str(&self, text: &str) -> String {
        let downgraded;
        let text = if self.config.has(Extension::Smart) {
            downgraded = downgrade_smart(text);
            downgraded.as_str()
        } else {
            text
        };
        let mut out = String::with_capacity(text.len());
        let mut prev: Option<char> = None;
        let mut backslash_run = 0usize;
        let mut iter = text.char_indices().peekable();
        while let Some((offset, ch)) = iter.next() {
            let next = iter.peek().map(|&(_, following)| following);
            let at_start = offset == 0;
            let word_start = at_start || prev.is_some_and(char::is_whitespace);
            let tail = || text.get(offset..).unwrap_or_default();
            backslash_run = if ch == '\\' { backslash_run + 1 } else { 0 };
            match ch {
                '#' if word_start && starts_heading(tail()) => out.push_str("\\#"),
                '!' if next == Some('[') => out.push_str("\\!"),
                '`' | '*' | '[' | ']' | '<' | '>' => {
                    out.push('\\');
                    out.push(ch);
                }
                '|' if self.config.has(Extension::PipeTables) => {
                    out.push('\\');
                    out.push(ch);
                }
                '$' if self.config.has(Extension::TexMathDollars) => {
                    out.push('\\');
                    out.push(ch);
                }
                '~' if self.config.has(Extension::Subscript) => {
                    out.push('\\');
                    out.push(ch);
                }
                '~' if self.config.has(Extension::Strikeout) && next == Some('~') => {
                    out.push('\\');
                    out.push(ch);
                }
                '^' if self.config.has(Extension::Superscript) => {
                    out.push('\\');
                    out.push(ch);
                }
                '@' if self.config.has(Extension::Citations) && word_start => out.push_str("\\@"),
                '&' if begins_character_reference(tail()) => out.push_str("\\&"),
                '&' if begins_named_entity(tail()) => out.push_str("\\&"),
                '_' if is_word_boundary(prev, next)
                    || !(self.config.cmark || self.config.has(Extension::IntrawordUnderscores)) =>
                {
                    out.push_str("\\_");
                }
                '\\' => self.escape_backslash(next, backslash_run, &mut out),
                other => out.push(other),
            }
            prev = Some(ch);
        }
        out
    }

    /// Escape a backslash. When raw TeX passes through verbatim every backslash is doubled so it is
    /// not mistaken for an escape. Otherwise a backslash is emitted verbatim except where a run of
    /// them ends the text with an odd length: the final one is then doubled so the run pads to an
    /// even number of backslashes and its last character is part of an escaped pair rather than a
    /// stray escape. `run_len` is the length of the backslash run ending at this character.
    fn escape_backslash(&self, next: Option<char>, run_len: usize, out: &mut String) {
        if self.config.has(Extension::RawTex) {
            out.push_str("\\\\");
            return;
        }
        out.push('\\');
        if next.is_none() && run_len % 2 == 1 {
            out.push('\\');
        }
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
        // Without the `footnotes` extension the dialect has no `[^n]` syntax, so a note degrades to
        // the generic numbered `[n]` reference and definition.
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

/// Encode the blank lines of a raw HTML fragment so it survives as one raw HTML block in markdown.
/// A blank line ends a raw HTML block, so the newline that opens one — any newline directly
/// following another — is rewritten as the `&#10;` character reference, leaving single line breaks
/// untouched. This keeps an HTML table embedded in a markdown dialect with no native table syntax
/// intact as a single raw block.
fn encode_html_block_blank_lines(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut prev_newline = false;
    for ch in html.chars() {
        if ch == '\n' && prev_newline {
            out.push_str("&#10;");
        } else {
            out.push(ch);
        }
        prev_newline = ch == '\n';
    }
    out
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

fn needs_separator(previous: &Block, current: &Block) -> bool {
    match (previous, current) {
        (Block::BulletList(_), Block::BulletList(_))
        | (Block::OrderedList(..), Block::OrderedList(..)) => true,
        (Block::BulletList(_) | Block::OrderedList(..), Block::CodeBlock(attr, _)) => {
            attr_is_empty(attr)
        }
        _ => false,
    }
}

fn offset_horizontal_rule(item: &[Block], body: String) -> String {
    if matches!(item.first(), Some(Block::HorizontalRule)) {
        format!("\n\n{body}")
    } else {
        body
    }
}

fn quote_block(body: &str) -> String {
    if body.is_empty() {
        return "> ".to_owned();
    }
    let mut out = String::new();
    for (index, line) in body.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        if line.is_empty() {
            out.push('>');
        } else {
            out.push_str("> ");
            out.push_str(line);
        }
    }
    out
}

fn is_html_format(format: &Format) -> bool {
    matches!(format.0.as_str(), "html" | "html4" | "html5")
}

/// Whether a raw-format name denotes TeX, which Markdown dialects with `raw_tex` embed verbatim.
/// `ConTeXt` and other TeX-adjacent formats are excluded — only `tex`/`latex` take the verbatim
/// path; everything else is rendered via the `raw_attribute` fenced form.
fn is_tex_format(format: &Format) -> bool {
    matches!(format.0.as_str(), "tex" | "latex")
}

fn collapse_trailing_newline(text: &str) -> String {
    text.strip_suffix('\n').unwrap_or(text).to_owned()
}

/// Escape a list marker that opens a paragraph, where it would otherwise start a list. Only the
/// paragraph's first token is at risk: a marker on a later line is a continuation of the paragraph,
/// not a list opener. A bullet marker (`-`/`+`) is escaped whenever it is the whole leading token;
/// an ordered marker (digits then `.`/`)`) is escaped only when a space or the line end follows, the
/// condition under which it would start a list.
fn escape_leading_markers(pieces: &mut [Piece]) {
    let break_follows = matches!(
        pieces.get(1),
        None | Some(Piece::Space | Piece::Soft | Piece::Hard)
    );
    let Some(Piece::Text(text)) = pieces.first_mut() else {
        return;
    };
    if let Some(escaped) = escaped_leading_marker(text, break_follows) {
        *text = escaped;
    }
}

/// The escaped form of a leading list marker, or `None` when the token is not one. A bullet token is
/// escaped unconditionally; an ordered token only when `break_follows` reports a space or line end
/// after it.
fn escaped_leading_marker(text: &str, break_follows: bool) -> Option<String> {
    if text == "-" || text == "+" {
        return Some(format!("\\{text}"));
    }
    let delim = text.chars().last()?;
    if !break_follows || (delim != '.' && delim != ')') {
        return None;
    }
    let digits = text
        .get(..text.len() - delim.len_utf8())
        .unwrap_or_default();
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(format!("{digits}\\{delim}"))
}

/// Whether a `#` run at the current position would open an ATX heading: one to six `#` followed by a
/// space or the end of the run.
fn starts_heading(text: &str) -> bool {
    let hashes = text.chars().take_while(|&c| c == '#').count();
    if hashes == 0 || hashes > 6 {
        return false;
    }
    matches!(text.chars().nth(hashes), None | Some(' '))
}

/// The info string for a fenced code block when fenced-code attributes are available, or `None` to
/// render it indented (no attributes). A lone class becomes a bare language tag; anything richer
/// uses the attribute block form.
fn extended_code_info(attr: &Attr) -> Option<String> {
    if attr_is_empty(attr) {
        return None;
    }
    if attr.id.is_empty()
        && attr.attributes.is_empty()
        && let [class] = attr.classes.as_slice()
    {
        return Some(format!(" {class}"));
    }
    Some(format!(" {}", attr_braces(attr)))
}

/// The info string for a fenced code block when fenced-code attributes are unavailable, or `None`
/// for indented output: only the first class survives, as a bare language tag.
fn github_code_info(attr: &Attr) -> Option<String> {
    match attr.classes.first() {
        Some(class) if !class.is_empty() => Some(format!(" {class}")),
        _ if attr_is_empty(attr) => None,
        _ => Some(String::new()),
    }
}

fn indent_code(text: &str) -> String {
    let body = text.trim_end_matches('\n');
    let mut out = String::new();
    for (index, line) in body.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        if !line.is_empty() {
            out.push_str("    ");
            out.push_str(line);
        }
    }
    out
}

/// The fence length for a fenced code block built from `fence`: longer than the longest leading run
/// of that character already in the body (so the fence cannot close early), and at least three.
fn fence_run_len(text: &str, fence: char) -> usize {
    let mut longest = 0;
    for line in text.split('\n') {
        let run = line.chars().take_while(|&c| c == fence).count();
        longest = longest.max(run);
    }
    (longest + 1).max(3)
}

/// The colon-fence length for a fenced div: longer than the longest leading colon run already in the
/// body (so a nested div's fence is strictly shorter), and at least three.
fn colon_fence_len(body: &str) -> usize {
    let mut longest = 0;
    for line in body.split('\n') {
        let run = line.chars().take_while(|&c| c == ':').count();
        longest = longest.max(run);
    }
    (longest + 1).max(3)
}

/// The text following a fenced-div opener: a bare class when the div carries only a single class and
/// the shorthand is allowed (`braced` is false), otherwise an attribute block.
fn div_opener(attr: &Attr, braced: bool) -> String {
    if !braced
        && attr.id.is_empty()
        && attr.attributes.is_empty()
        && let [class] = attr.classes.as_slice()
    {
        return format!(" {class}");
    }
    format!(" {}", attr_braces(attr))
}

/// Replace smart-punctuation glyphs with their ASCII equivalents for a dialect that does not write
/// them: ellipsis, en/em dashes, and curly quotes.
fn downgrade_smart(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '…' => out.push_str("..."),
            '—' => out.push_str("---"),
            '–' => out.push_str("--"),
            '“' | '”' => out.push('"'),
            '‘' | '’' => out.push('\''),
            other => out.push(other),
        }
    }
    out
}

fn code_span(text: &str) -> String {
    let max_run = longest_backtick_run(text);
    let fence = "`".repeat((max_run + 1).max(1));
    let needs_padding = max_run > 0
        || (text.starts_with(' ') && text.ends_with(' ') && text.chars().any(|ch| ch != ' '));
    if needs_padding {
        format!("{fence} {text} {fence}")
    } else {
        format!("{fence}{text}{fence}")
    }
}

fn longest_backtick_run(text: &str) -> usize {
    let mut longest = 0;
    let mut current = 0;
    for ch in text.chars() {
        if ch == '`' {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    longest
}

fn destination(target: &Target) -> String {
    if target.title.is_empty() {
        target.url.clone()
    } else {
        format!("{} \"{}\"", target.url, escape_title(&target.title))
    }
}

fn escape_title(title: &str) -> String {
    title.replace('"', "\\\"")
}

fn autolink(inlines: &[Inline], target: &Target) -> Option<String> {
    let [Inline::Str(text)] = inlines else {
        return None;
    };
    if &target.url == text && is_uri(text) {
        return Some(format!("<{text}>"));
    }
    if target.url == format!("mailto:{text}") {
        return Some(format!("<{text}>"));
    }
    None
}

fn is_uri(text: &str) -> bool {
    let Some(colon) = text.find(':') else {
        return false;
    };
    text.get(..colon).is_some_and(is_known_scheme) && is_percent_escaped_uri(text, true)
}

fn attr_is_empty(attr: &Attr) -> bool {
    attr.id.is_empty() && attr.classes.is_empty() && attr.attributes.is_empty()
}

/// Flatten a figure caption's blocks into one inline sequence for the implicit-figure form: each
/// paragraph contributes its inlines and successive paragraphs are joined by a line break. An empty
/// caption yields an empty sequence. Returns `None` if any block is not a paragraph, leaving a
/// richly structured caption to fall back to an HTML figure.
fn caption_blocks_as_inlines(blocks: &[Block]) -> Option<Vec<Inline>> {
    let mut inlines = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        let (Block::Plain(paragraph) | Block::Para(paragraph)) = block else {
            return None;
        };
        if index > 0 {
            inlines.push(Inline::LineBreak);
        }
        inlines.extend(paragraph.iter().cloned());
    }
    Some(inlines)
}

/// Whether a header's attributes are exactly the identifier a reader would derive from its text, so
/// the explicit `{#id}` block is redundant and can be dropped.
fn header_attr_implicit(attr: &Attr, inlines: &[Inline], auto_identifiers: bool) -> bool {
    attr.classes.is_empty()
        && attr.attributes.is_empty()
        && (attr.id.is_empty()
            || (auto_identifiers && attr.id == carta_ast::slug(&carta_ast::to_plain_text(inlines))))
}

fn is_autolink_class(attr: &Attr) -> bool {
    attr.id.is_empty()
        && attr.attributes.is_empty()
        && matches!(attr.classes.as_slice(), [class] if class == "uri" || class == "email")
}

fn has_dimension(attr: &Attr) -> bool {
    attr.attributes
        .iter()
        .any(|(key, _)| matches!(key.as_str(), "width" | "height"))
}

fn title_attr(title: &Text) -> String {
    if title.is_empty() {
        String::new()
    } else {
        format!(" title=\"{}\"", escape_attr(title))
    }
}

/// An image rendered as an HTML `<img>` element (the fallback for an image carrying attributes when
/// link attributes have no native syntax).
fn image_html(attr: &Attr, inlines: &[Inline], target: &Target) -> String {
    let alt = carta_ast::to_plain_text(inlines);
    let alt_attr = if alt.is_empty() {
        String::new()
    } else {
        format!(" alt=\"{}\"", escape_attr(&alt))
    };
    format!(
        "<img src=\"{}\"{}{}{alt_attr} />",
        escape_attr(&target.url),
        title_attr(&target.title),
        render_html_attr(attr),
    )
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

fn underline_attr() -> Attr {
    Attr {
        classes: vec!["underline".to_owned()],
        ..Attr::default()
    }
}

fn smallcaps_attr() -> Attr {
    Attr {
        classes: vec!["smallcaps".to_owned()],
        ..Attr::default()
    }
}

/// The straight ASCII quote glyphs for a quote kind, used when the dialect downgrades smart
/// punctuation.
fn ascii_quote_marks(kind: &carta_ast::QuoteType) -> (char, char) {
    match kind {
        carta_ast::QuoteType::SingleQuote => ('\'', '\''),
        carta_ast::QuoteType::DoubleQuote => ('"', '"'),
    }
}

/// Whether a list item's first block opens with a checkbox glyph, and if so whether it is checked.
/// `None` when the item is not a checkbox item.
fn checkbox_state(item: &[Block]) -> Option<bool> {
    let (Block::Plain(inlines) | Block::Para(inlines)) = item.first()? else {
        return None;
    };
    match inlines.first()? {
        Inline::Str(text) if text == "\u{2610}" => Some(false),
        Inline::Str(text) if text == "\u{2612}" => Some(true),
        _ => None,
    }
}

/// Remove the leading checkbox glyph and the space after it from a list item's first block.
fn strip_checkbox(item: &[Block]) -> Vec<Block> {
    let mut blocks = item.to_vec();
    if let Some(Block::Plain(inlines) | Block::Para(inlines)) = blocks.first_mut()
        && matches!(inlines.first(), Some(Inline::Str(text)) if text == "\u{2610}" || text == "\u{2612}")
    {
        inlines.remove(0);
        if matches!(inlines.first(), Some(Inline::Space)) {
            inlines.remove(0);
        }
    }
    blocks
}

/// Whether a table can render as a pipe table: every cell holds at most one paragraph of inline
/// content that fits on a single line (no forced break), and no cell spans more than one row or
/// column. A table failing this falls back to an HTML render.
fn pipe_representable(table: &Table) -> bool {
    let rows = table
        .head
        .rows
        .iter()
        .chain(body_rows(table))
        .chain(table.foot.rows.iter());
    rows.flat_map(|row| row.cells.iter()).all(|cell| {
        cell.row_span <= 1
            && cell.col_span <= 1
            && is_simple_cell(cell)
            && !cell_inlines(cell)
                .iter()
                .any(|inline| matches!(inline, Inline::LineBreak))
    })
}

/// Render a table's attributes as a trailing `{#id .class key="value"}` suffix, or `None` when the
/// table carries no attributes.
fn attribute_suffix(attr: &Attr) -> Option<String> {
    if attr_is_empty(attr) {
        return None;
    }
    Some(attr_braces(attr))
}

/// One pipe-table row: each cell padded to its column width and wrapped in `| … |`. Alignment
/// controls the padding side.
fn pipe_row(cells: &[String], widths: &[usize], aligns: &[Alignment]) -> String {
    let mut out = String::from("|");
    for (index, width) in widths.iter().enumerate() {
        let text = cells.get(index).map_or("", String::as_str);
        let align = aligns
            .get(index)
            .cloned()
            .unwrap_or(Alignment::AlignDefault);
        out.push(' ');
        out.push_str(&pad_align(text, *width, &align));
        out.push_str(" |");
    }
    out
}

/// The pipe-table alignment separator row: a dash run per column, with colons marking each column's
/// alignment, padded to the column width.
fn pipe_separator(widths: &[usize], aligns: &[Alignment]) -> String {
    let mut out = String::from("|");
    for (index, &width) in widths.iter().enumerate() {
        let align = aligns
            .get(index)
            .cloned()
            .unwrap_or(Alignment::AlignDefault);
        out.push_str(&pipe_dashes(width, &align));
        out.push('|');
    }
    out
}

/// One column's alignment-separator field: a dash run spanning the column's full interior width
/// (`width + 2`, matching a content field's surrounding spaces), with colons replacing the edge
/// dashes per alignment and no surrounding padding.
fn pipe_dashes(width: usize, align: &Alignment) -> String {
    let interior = width + 2;
    let mut field = String::with_capacity(interior);
    match align {
        Alignment::AlignLeft => {
            field.push(':');
            field.push_str(&"-".repeat(interior.saturating_sub(1)));
        }
        Alignment::AlignRight => {
            field.push_str(&"-".repeat(interior.saturating_sub(1)));
            field.push(':');
        }
        Alignment::AlignCenter => {
            field.push(':');
            field.push_str(&"-".repeat(interior.saturating_sub(2)));
            field.push(':');
        }
        Alignment::AlignDefault => field.push_str(&"-".repeat(interior)),
    }
    field
}

fn begins_character_reference(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.first() != Some(&b'&') || bytes.get(1) != Some(&b'#') {
        return false;
    }
    let hex = matches!(bytes.get(2), Some(b'x' | b'X'));
    let start = if hex { 3 } else { 2 };
    let mut pos = start;
    while bytes.get(pos).is_some_and(|byte| {
        if hex {
            byte.is_ascii_hexdigit()
        } else {
            byte.is_ascii_digit()
        }
    }) {
        pos += 1;
    }
    pos > start && bytes.get(pos) == Some(&b';')
}

fn begins_named_entity(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.first() != Some(&b'&') {
        return false;
    }
    if !bytes.get(1).is_some_and(u8::is_ascii_alphabetic) {
        return false;
    }
    let mut pos = 2;
    while bytes.get(pos).is_some_and(u8::is_ascii_alphanumeric) {
        pos += 1;
    }
    if bytes.get(pos) != Some(&b';') {
        return false;
    }
    let name = text.get(1..pos).unwrap_or_default();
    entity_names::ENTITY_NAMES.binary_search(&name).is_ok()
}

mod entity_names {
    include!(concat!(env!("OUT_DIR"), "/entity_names.rs"));
}

fn is_word_boundary(before: Option<char>, after: Option<char>) -> bool {
    let alnum = |ch: Option<char>| ch.is_some_and(char::is_alphanumeric);
    !(alnum(before) && alnum(after))
}

#[cfg(test)]
mod tests {
    use super::{yaml_inline_scalar, yaml_needs_quoting};

    #[test]
    fn yaml_quotes_only_scalars_that_would_reparse_wrongly() {
        // A colon, a ` #` comment opener, a leading indicator, surrounding space, emptiness, and a
        // bool/null keyword each force quoting.
        for forced in [
            "Chapter 1: The Beginning",
            "a:b",
            "ends:",
            "http://example.com",
            "has #comment",
            "-leading",
            "#leading",
            "@leading",
            "!leading",
            "%leading",
            " leading",
            "trailing ",
            "",
            "true",
            "False",
            "NULL",
            "yes",
            "off",
        ] {
            assert!(
                yaml_needs_quoting(forced),
                "expected quoting for {forced:?}"
            );
        }

        // Plain text, interior punctuation that stays valid bare, numbers, and non-keyword words are
        // left unquoted.
        for bare in [
            "plain words",
            "interior-dash here",
            "comma,here",
            "has \" quote",
            "back\\slash",
            "123",
            "1.5",
            "None",
            "under_score",
        ] {
            assert!(
                !yaml_needs_quoting(bare),
                "expected no quoting for {bare:?}"
            );
        }
    }

    #[test]
    fn yaml_quote_escapes_backslash_and_quote() {
        assert_eq!(yaml_inline_scalar("a: b"), "\"a: b\"");
        assert_eq!(yaml_inline_scalar("a \" b"), "a \" b");
        assert_eq!(yaml_inline_scalar(": x\\y"), "\": x\\\\y\"");
        assert_eq!(yaml_inline_scalar("plain"), "plain");
    }

    mod columns {
        use carta_ast::{Block, Document, Inline};
        use carta_core::{Writer, WriterOptions};

        use crate::markdown::MarkdownWriter;

        fn long_paragraph() -> Vec<Block> {
            let words: Vec<Inline> =
                "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu"
                    .split(' ')
                    .flat_map(|word| [Inline::Str(word.to_owned()), Inline::Space])
                    .collect();
            vec![Block::Para(words)]
        }

        fn render(blocks: Vec<Block>, columns: Option<usize>) -> String {
            let document = Document {
                blocks,
                ..Document::default()
            };
            let mut options = WriterOptions::default();
            options.columns = columns;
            MarkdownWriter.write(&document, &options).unwrap()
        }

        #[test]
        fn narrow_columns_wraps_a_paragraph_sooner() {
            let narrow = render(long_paragraph(), Some(20));
            let wide = render(long_paragraph(), Some(60));
            assert!(narrow.lines().count() > wide.lines().count());
            assert!(narrow.lines().all(|line| line.chars().count() <= 20));
            assert!(wide.lines().all(|line| line.chars().count() <= 60));
        }

        #[test]
        fn omitted_columns_uses_the_default_fill_width() {
            assert_eq!(
                render(long_paragraph(), None),
                render(long_paragraph(), Some(72))
            );
        }
    }

    mod raw_blocks {
        use carta_ast::{Block, Document, Format};
        use carta_core::{Writer, WriterOptions};

        use crate::markdown::MarkdownWriter;

        // A raw-attribute block opens with a backtick fence longer than any run inside its body, so a
        // body that itself contains a ``` line cannot close the fence early.
        #[test]
        fn raw_attribute_fence_outgrows_a_backtick_run_in_the_body() {
            let document = Document {
                blocks: vec![Block::RawBlock(
                    Format("dot".to_owned()),
                    "```\ngraph {}\n```".to_owned(),
                )],
                ..Document::default()
            };
            let output = MarkdownWriter
                .write(&document, &WriterOptions::default())
                .unwrap();
            assert_eq!(output, "````{=dot}\n```\ngraph {}\n```\n````");
        }
    }

    mod escaping {
        use carta_ast::{Block, Document, Inline, MathType};
        use carta_core::{Extension, Extensions, Writer, WriterOptions, presets};

        use crate::markdown::MarkdownWriter;

        fn s(text: &str) -> Inline {
            Inline::Str(text.to_owned())
        }

        fn render(blocks: Vec<Block>) -> String {
            render_with(blocks, presets::MARKDOWN)
        }

        fn render_with(blocks: Vec<Block>, extensions: Extensions) -> String {
            let document = Document {
                blocks,
                ..Document::default()
            };
            let mut options = WriterOptions::default();
            options.extensions = extensions;
            MarkdownWriter.write(&document, &options).unwrap()
        }

        fn without(ext: Extension) -> Extensions {
            let mut extensions = presets::MARKDOWN;
            extensions.remove(ext);
            extensions
        }

        #[test]
        fn a_leading_ordered_marker_is_escaped_only_when_a_list_would_open() {
            // A digit run then `.`/`)` followed by a space or the line end would open a list.
            assert_eq!(
                render(vec![Block::Para(vec![s("1."), Inline::Space, s("Item")])]),
                "1\\. Item"
            );
            assert_eq!(
                render(vec![Block::Para(vec![s("1)"), Inline::Space, s("Item")])]),
                "1\\) Item"
            );
            assert_eq!(
                render(vec![Block::Para(vec![s("12."), Inline::Space, s("Item")])]),
                "12\\. Item"
            );
            assert_eq!(render(vec![Block::Para(vec![s("1.")])]), "1\\.");
            // No following break: the token cannot start a list and stays bare.
            assert_eq!(render(vec![Block::Para(vec![s("1.Item")])]), "1.Item");
        }

        #[test]
        fn a_leading_bullet_marker_is_escaped() {
            assert_eq!(
                render(vec![Block::Para(vec![s("-"), Inline::Space, s("x")])]),
                "\\- x"
            );
            assert_eq!(
                render(vec![Block::Para(vec![s("+"), Inline::Space, s("x")])]),
                "\\+ x"
            );
            // A plain block (e.g. a tight list item) is at the same risk.
            assert_eq!(
                render(vec![Block::Plain(vec![s("-"), Inline::Space, s("x")])]),
                "\\- x"
            );
        }

        #[test]
        fn a_marker_past_the_first_token_is_left_alone() {
            // Only the opening token can start a list; a marker on a wrapped continuation cannot.
            assert_eq!(
                render(vec![Block::Para(vec![
                    s("text"),
                    Inline::SoftBreak,
                    s("-"),
                    Inline::Space,
                    s("x"),
                ])]),
                "text - x"
            );
        }

        #[test]
        fn a_double_tilde_is_escaped_under_strikeout() {
            // With subscript off, only the strikeout-opening tilde of each pair is escaped.
            assert_eq!(
                render_with(
                    vec![Block::Para(vec![s("~~foo~~")])],
                    without(Extension::Subscript)
                ),
                "\\~~foo\\~~"
            );
            // With strikeout also off, the tildes are literal.
            let mut bare = presets::MARKDOWN;
            bare.remove(Extension::Subscript);
            bare.remove(Extension::Strikeout);
            assert_eq!(
                render_with(vec![Block::Para(vec![s("~~foo~~")])], bare),
                "~~foo~~"
            );
        }

        #[test]
        fn a_trailing_backslash_run_pads_to_an_even_length() {
            // With raw-TeX passthrough off, a backslash run ending the text doubles its last member
            // only when the run is odd, so the run never ends on a stray escape.
            let exts = without(Extension::RawTex);
            assert_eq!(
                render_with(vec![Block::Para(vec![s("a\\")])], exts),
                "a\\\\"
            );
            assert_eq!(
                render_with(vec![Block::Para(vec![s("a\\\\")])], exts),
                "a\\\\"
            );
            assert_eq!(
                render_with(vec![Block::Para(vec![s("a\\\\\\")])], exts),
                "a\\\\\\\\"
            );
            // An interior backslash is emitted verbatim.
            assert_eq!(
                render_with(vec![Block::Para(vec![s("a\\b")])], exts),
                "a\\b"
            );
        }

        #[test]
        fn the_backslash_math_surfaces_wrap_the_expression() {
            let single = {
                let mut exts = presets::MARKDOWN;
                exts.remove(Extension::TexMathDollars);
                exts.insert(Extension::TexMathSingleBackslash);
                exts
            };
            assert_eq!(
                render_with(
                    vec![Block::Para(vec![Inline::Math(
                        MathType::InlineMath,
                        "x^2".to_owned()
                    )])],
                    single
                ),
                "\\(x^2\\)"
            );
            assert_eq!(
                render_with(
                    vec![Block::Para(vec![Inline::Math(
                        MathType::DisplayMath,
                        "x^2".to_owned()
                    )])],
                    single
                ),
                "\\[x^2\\]"
            );
            let double = {
                let mut exts = presets::MARKDOWN;
                exts.remove(Extension::TexMathDollars);
                exts.insert(Extension::TexMathDoubleBackslash);
                exts
            };
            assert_eq!(
                render_with(
                    vec![Block::Para(vec![Inline::Math(
                        MathType::InlineMath,
                        "x^2".to_owned()
                    )])],
                    double
                ),
                "\\\\(x^2\\\\)"
            );
        }

        #[test]
        fn an_unwritable_display_math_falls_back_on_its_own_line() {
            // With no math surface, a display expression linearizes to markup set off by line breaks.
            assert_eq!(
                render_with(
                    vec![Block::Para(vec![
                        s("before"),
                        Inline::Space,
                        Inline::Math(MathType::DisplayMath, "x^2".to_owned()),
                        Inline::Space,
                        s("after"),
                    ])],
                    without(Extension::TexMathDollars),
                ),
                "before\n*x*^2^\nafter"
            );
        }
    }
}
