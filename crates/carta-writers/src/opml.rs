//! Outline writer: renders the document model to a nested outline of `<outline>` elements.
//!
//! The block sequence is sectioned by headers into a tree: each header opens an outline whose
//! `text` attribute is the header's inlines rendered as inline html, and whose `_note` attribute is
//! the markdown render of the blocks that follow the header up to its first subsection. A header
//! with a greater level number nests as a child outline; siblings and shallower headers close the
//! current outline. Content before the first header has no outline to attach to and is dropped.
//! Output carries no trailing newline; the caller appends one.

use carta_ast::{Block, Document, Inline};
use carta_core::{Result, WrapMode, Writer, WriterOptions};

use crate::common::FILL_COLUMN;
use crate::html::render_inline_line;
use crate::markdown::{MarkdownConfig, render_blocks};

/// Renders a document to a nested outline.
#[derive(Debug, Default, Clone, Copy)]
pub struct OpmlWriter;

impl Writer for OpmlWriter {
    fn write(&self, document: &Document, options: &WriterOptions) -> Result<String> {
        let width = options.columns.unwrap_or(FILL_COLUMN);
        let sections = sectionize(&document.blocks);
        let mut out = String::new();
        for section in &sections {
            render_section(section, 0, width, options.wrap, &mut out);
        }
        Ok(out.trim_end_matches('\n').to_owned())
    }

    fn default_template(&self) -> Option<&'static str> {
        Some(include_str!("templates/default.opml"))
    }

    fn render_meta_inlines(&self, inlines: &[Inline], options: &WriterOptions) -> Result<String> {
        Ok(render_blocks(
            &[Block::Plain(inlines.to_vec())],
            MarkdownConfig::extended(),
            options.columns.unwrap_or(FILL_COLUMN),
            options.wrap,
        ))
    }

    fn render_meta_blocks(&self, blocks: &[Block], options: &WriterOptions) -> Result<String> {
        Ok(render_blocks(
            blocks,
            MarkdownConfig::extended(),
            options.columns.unwrap_or(FILL_COLUMN),
            options.wrap,
        ))
    }
}

/// One header and the document tree rooted at it: the blocks directly under the header (before any
/// subsection) and the subsections nested beneath it.
struct Section<'a> {
    heading: &'a [Inline],
    body: Vec<&'a Block>,
    children: Vec<Section<'a>>,
}

/// Split a block sequence into the top-level sections it contains. Blocks before the first header
/// belong to no section and are discarded.
fn sectionize(blocks: &[Block]) -> Vec<Section<'_>> {
    let mut index = 0;
    while index < blocks.len() {
        if matches!(blocks.get(index), Some(Block::Header(..))) {
            break;
        }
        index += 1;
    }
    let mut sections = Vec::new();
    while index < blocks.len() {
        let Some(Block::Header(level, _, heading)) = blocks.get(index) else {
            index += 1;
            continue;
        };
        let level = *level;
        let start = index + 1;
        let mut end = start;
        while let Some(block) = blocks.get(end) {
            if let Block::Header(next_level, _, _) = block
                && *next_level <= level
            {
                break;
            }
            end += 1;
        }
        let inner = blocks.get(start..end).unwrap_or_default();
        let mut body = Vec::new();
        let mut child_start = inner.len();
        for (offset, block) in inner.iter().enumerate() {
            if matches!(block, Block::Header(..)) {
                child_start = offset;
                break;
            }
            body.push(block);
        }
        let children = sectionize(inner.get(child_start..).unwrap_or_default());
        sections.push(Section {
            heading,
            body,
            children,
        });
        index = end;
    }
    sections
}

fn render_section(section: &Section, depth: usize, width: usize, wrap: WrapMode, out: &mut String) {
    let indent = "  ".repeat(depth);
    let text = escape_outline(&render_inline_line(section.heading));
    let body_blocks: Vec<Block> = section.body.iter().map(|block| (*block).clone()).collect();
    let note = render_blocks(&body_blocks, MarkdownConfig::extended(), width, wrap);
    out.push_str(&indent);
    out.push_str("<outline text=\"");
    out.push_str(&text);
    out.push('"');
    if !note.is_empty() {
        out.push_str(" _note=\"");
        out.push_str(&escape_outline(&note));
        out.push('"');
    }
    out.push_str(">\n");
    for child in &section.children {
        render_section(child, depth + 1, width, wrap, out);
    }
    out.push_str(&indent);
    out.push_str("</outline>\n");
}

/// Escape a string for an XML attribute value: the metacharacters `& < > "` to their entities and a
/// newline to its numeric reference, so a multi-line markdown note stays on one logical line.
fn escape_outline(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\n' => out.push_str("&#10;"),
            other => out.push(other),
        }
    }
    out
}
