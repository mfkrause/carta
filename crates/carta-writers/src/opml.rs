//! Render the document model as nested `<outline>` elements.

use carta_ast::{Attr, Block, Document, Inline, Text};
use carta_core::sections::{SECTION_CLASS, section_heading, section_opening};
use carta_core::{Result, WrapMode, Writer, WriterOptions, with_stack};

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
    body: &'a [Block],
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
            body: inner.get(..child_start).unwrap_or_default(),
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
    let note = render_blocks(
        &section_divisions(section.body),
        MarkdownConfig::extended(),
        width,
        wrap,
    );
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

/// Copy note blocks, wrapping each header that sits inside a division, with the blocks it governs,
/// in a section division. The section takes the header's identifier and repeats its classes and
/// attributes after the section class. Only divisions are searched for headers.
fn section_divisions(blocks: &[Block]) -> Vec<Block> {
    let mut out = Vec::with_capacity(blocks.len());
    let mut index = 0;
    while let Some(block) = blocks.get(index) {
        index += 1;
        match block {
            Block::Header(level, attr, title) => {
                let start = index;
                while blocks.get(index).is_some_and(|next| {
                    section_opening(next).is_none_or(|opening| opening.level > *level)
                }) {
                    index += 1;
                }
                let mut classes = vec![Text::from(SECTION_CLASS)];
                classes.extend(attr.classes.iter().cloned());
                let section = Attr {
                    id: attr.id.clone(),
                    classes,
                    attributes: attr.attributes.clone(),
                };
                let heading = Attr {
                    id: Text::default(),
                    ..(**attr).clone()
                };
                let mut children = vec![Block::Header(*level, Box::new(heading), title.clone())];
                children.extend(section_divisions(
                    blocks.get(start..index).unwrap_or_default(),
                ));
                out.push(Block::Div(Box::new(section), children));
            }
            Block::Div(attr, inner) => {
                // A division that already is a section keeps its own header.
                let kept = usize::from(section_heading(attr, inner).is_some());
                let mut children: Vec<Block> = inner.iter().take(kept).cloned().collect();
                children.extend(with_stack(|| {
                    section_divisions(inner.get(kept..).unwrap_or_default())
                }));
                out.push(Block::Div(attr.clone(), children));
            }
            other => out.push(other.clone()),
        }
    }
    out
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
