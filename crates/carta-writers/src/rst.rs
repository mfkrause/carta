//! reStructuredText writer: renders the document model to RST source text.
//!
//! Block structure is conveyed through directives and a three-space indent; inline emphasis maps to
//! `*`/`**`, inline code to double backticks, and roles such as `:sup:` and `:math:`. Footnotes and
//! image substitutions are collected while rendering and emitted as definition sections after the
//! body. Output carries no trailing newline; the caller appends one. Content is wrapped at a fill
//! column of 72.

use carta_ast::{Block, Document, Inline, MetaValue, single_block_inlines};
use carta_core::{Extension, Result, TocStyle, WrapMode, Writer, WriterOptions};

use crate::common::{FILL_COLUMN, display_width, fill, fill_cell, fill_hang, indent_block};

use self::block::{block_separator, code_block, raw_block};
use self::inline::{flatten, to_pieces};

mod block;
mod inline;
mod table;

/// Width of the transition emitted for a horizontal rule.
const RULE_WIDTH: usize = 14;

/// Renders a document to reStructuredText.
#[derive(Debug, Default, Clone, Copy)]
pub struct RstWriter;

impl Writer for RstWriter {
    fn write(&self, document: &Document, options: &WriterOptions) -> Result<String> {
        let width = options.columns.unwrap_or(FILL_COLUMN);
        let mut state = State {
            wrap: options.wrap,
            width,
            smart: options.extensions.contains(Extension::Smart),
            ..State::default()
        };
        let body = state.blocks_to_string(&document.blocks, width, true);
        let mut sections = Vec::new();
        if !body.is_empty() {
            sections.push(body);
        }
        let notes: Vec<String> = state
            .footnotes
            .iter()
            .filter(|entry| !entry.is_empty())
            .cloned()
            .collect();
        if !notes.is_empty() {
            sections.push(notes.join("\n\n"));
        }
        if !state.substitutions.is_empty() {
            sections.push(state.substitutions.join("\n"));
        }
        Ok(sections.join("\n\n"))
    }

    fn default_template(&self) -> Option<&'static str> {
        Some(include_str!("templates/default.rst"))
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

    fn title_block(&self, document: &Document, _options: &WriterOptions) -> Result<Option<String>> {
        let mut parts = Vec::new();
        if let Some(line) = title_line(document.meta.get("title")) {
            let bar = "=".repeat(display_width(&line));
            parts.push(format!("{bar}\n{line}\n{bar}"));
        }
        if let Some(line) = title_line(document.meta.get("subtitle")) {
            let bar = "-".repeat(display_width(&line));
            parts.push(format!("{bar}\n{line}\n{bar}"));
        }
        Ok((!parts.is_empty()).then(|| parts.join("\n")))
    }
}

/// Render a metadata value to a single reStructuredText title line, or `None` when it carries no
/// text. The line feeds an over/underlined title, whose rule length matches its display width.
fn title_line(value: Option<&MetaValue>) -> Option<String> {
    let inlines = match value? {
        MetaValue::MetaInlines(inlines) => inlines.clone(),
        MetaValue::MetaString(text) => vec![Inline::Str(text.clone())],
        MetaValue::MetaBlocks(blocks) => single_block_inlines(blocks).to_vec(),
        _ => return None,
    };
    let mut state = State::default();
    let line = flatten(state.tokens(&inlines)).trim().to_owned();
    (!line.is_empty()).then_some(line)
}

/// Collects the deferred constructs accumulated during rendering: footnote definitions and image
/// substitution definitions, both emitted as their own sections after the document body. The counter
/// names images that carry no alt text.
#[derive(Debug)]
struct State {
    footnotes: Vec<String>,
    substitutions: Vec<String>,
    fallback_count: usize,
    /// Substitution names already assigned, in assignment order. A repeated label falls back to a
    /// generated `image`-plus-counter name so each reference resolves to its own definition.
    used_names: Vec<String>,
    wrap: WrapMode,
    /// The fill column the document body lays out to.
    width: usize,
    /// Set while laying out the content of a table cell, whose field reflows to its column width
    /// even when the document is not auto-wrapped.
    in_cell: bool,
    /// Whether `smart` punctuation is rendered: quotes become straight ASCII and Unicode dashes and
    /// the ellipsis collapse to their ASCII forms, rather than passing through as literal Unicode.
    smart: bool,
    /// How many tables the current render is nested inside, counting the one being rendered.
    table_depth: usize,
}

impl Default for State {
    fn default() -> Self {
        Self {
            footnotes: Vec::new(),
            substitutions: Vec::new(),
            fallback_count: 0,
            used_names: Vec::new(),
            wrap: WrapMode::default(),
            width: FILL_COLUMN,
            in_cell: false,
            smart: false,
            table_depth: 0,
        }
    }
}

impl State {
    /// Render a block sequence into the document's default layout. Consecutive blocks are separated
    /// by a blank line, except that a [`Block::Plain`] is followed by a single newline when the next
    /// block can sit directly beneath it (see [`tight_after_plain`]). Blocks that render empty are
    /// dropped.
    fn blocks_to_string(&mut self, blocks: &[Block], width: usize, top: bool) -> String {
        self.blocks_laid(blocks, width, top, false)
    }

    /// Render a block sequence as [`Self::blocks_to_string`], but when `hang` is set the first
    /// non-empty block keeps a space that opens it, so a list item's text keeps the gap the source
    /// put after the marker rather than collapsing it against the marker.
    fn blocks_laid(&mut self, blocks: &[Block], width: usize, top: bool, hang: bool) -> String {
        let mut out = String::new();
        let mut previous: Option<&Block> = None;
        let mut first = true;
        for block in blocks {
            let text = self.block_laid(block, width, top, hang && first);
            if text.is_empty() {
                continue;
            }
            if let Some(prev) = previous {
                out.push_str(block_separator(prev, block));
            }
            out.push_str(&text);
            previous = Some(block);
            first = false;
        }
        out
    }

    /// Fill inline content to `width` under the active wrap mode. Inside a table cell the field
    /// reflows to its column width even when the document is not auto-wrapped. With `hang`, a space
    /// that opens the content is kept rather than dropped.
    fn lay(&mut self, inlines: &[Inline], width: usize, hang: bool) -> String {
        let pieces = to_pieces(self.tokens(inlines));
        if self.in_cell {
            fill_cell(&pieces, width, self.wrap)
        } else if hang {
            fill_hang(&pieces, width, self.wrap)
        } else {
            fill(&pieces, width, self.wrap)
        }
    }

    fn block_laid(&mut self, block: &Block, width: usize, top: bool, hang: bool) -> String {
        match block {
            Block::Plain(inlines) => self.lay(inlines, width, hang),
            Block::Para(inlines) => self.para(inlines, width, hang),
            Block::Header(level, attr, inlines) => self.header(*level, attr, inlines, top),
            Block::CodeBlock(attr, text) => code_block(attr, text),
            Block::RawBlock(format, text) => raw_block(format, text),
            Block::BlockQuote(blocks) => {
                let body = self.blocks_to_string(blocks, width.saturating_sub(3), false);
                if body.is_empty() {
                    String::new()
                } else {
                    indent_block(&body, "   ", "   ")
                }
            }
            Block::BulletList(items) => self.bullet_list(items, width),
            Block::OrderedList(attrs, items) => self.ordered_list(attrs, items, width),
            Block::DefinitionList(items) => self.definition_list(items, width),
            Block::HorizontalRule => "-".repeat(RULE_WIDTH),
            Block::Table(table) => self.table(table, width),
            Block::Figure(attr, caption, blocks) => self.figure(attr, caption, blocks, width),
            Block::Div(attr, blocks) => self.div(attr, blocks, width),
            Block::LineBlock(lines) => {
                let rendered: Vec<String> = lines
                    .iter()
                    .map(|line| self.render_line(line, width))
                    .collect();
                rendered.join("\n")
            }
        }
    }
}

#[cfg(test)]
mod tests;
