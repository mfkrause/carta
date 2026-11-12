//! Render the document model as nested `<outline>` elements.

use carta_ast::{Block, Document, Inline};
use carta_core::sections::section_opening;
use carta_core::{Result, WrapMode, Writer, WriterOptions};

use crate::common::FILL_COLUMN;
use crate::html::render_fragment;
use crate::markdown::{MarkdownConfig, render_blocks};

/// Renders a document to a nested outline.
#[derive(Debug, Default, Clone, Copy)]
pub struct OpmlWriter;

impl Writer for OpmlWriter {
    fn render_document(&self, document: &Document, options: &WriterOptions) -> Result<String> {
        let width = options.columns.unwrap_or(FILL_COLUMN);
        let blocks = carta_core::sections::merge_heading_divs(&document.blocks);
        let sections = sectionize(&blocks);
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

/// Split a block sequence into the top-level sections it contains. Blocks before the first section
/// belong to no section and are discarded.
fn sectionize(blocks: &[Block]) -> Vec<Section<'_>> {
    let mut sections = Vec::new();
    let mut index = blocks
        .iter()
        .position(|block| section_opening(block).is_some())
        .unwrap_or(blocks.len());
    while let Some(block) = blocks.get(index) {
        let Some(open) = section_opening(block) else {
            index += 1;
            continue;
        };
        let inner = if let Some(body) = open.body {
            index += 1;
            body
        } else {
            let start = index + 1;
            let mut end = start;
            while let Some(next) = blocks.get(end) {
                if section_opening(next).is_some_and(|next| next.level <= open.level) {
                    break;
                }
                end += 1;
            }
            index = end;
            blocks.get(start..end).unwrap_or_default()
        };
        let child_start = inner
            .iter()
            .position(|block| section_opening(block).is_some())
            .unwrap_or(inner.len());
        sections.push(Section {
            heading: open.title,
            body: inner
                .get(..child_start)
                .unwrap_or_default()
                .iter()
                .collect(),
            children: sectionize(inner.get(child_start..).unwrap_or_default()),
        });
    }
    sections
}

fn render_section(section: &Section, depth: usize, width: usize, wrap: WrapMode, out: &mut String) {
    let indent = "  ".repeat(depth);
    // An outline title is always filled to the default column, whatever the wrap setting.
    let title = render_fragment(&[Block::Plain(section.heading.to_vec())], WrapMode::Auto);
    let text = escape_outline(&title);
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
