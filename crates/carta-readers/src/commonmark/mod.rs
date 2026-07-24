//! `CommonMark` reader.
//!
//! Parsing follows the spec's two-phase strategy: the block phase (`block`) consumes the input
//! line by line into a tree of `IrBlock`s whose leaves still hold raw text, collecting link
//! reference definitions; the inline phase (`inline`) then parses each leaf's text into inlines.
//! The result is assembled into a [`Document`].

mod attr;
mod autolink;
mod block;
mod cursor;
mod emphasis;
mod frontmatter;
mod grid;
mod html_block;
mod html_element;
mod identifiers;
mod inline;
mod postprocess;
mod resolve;
pub(crate) mod scan;
mod table;
mod texttable;
mod yaml;

use std::borrow::Cow;
use std::collections::BTreeMap;

use carta_ast::{Alignment, Attr, Block, Document, Format, Inline, ListAttributes};
use carta_core::{Extensions, Reader, ReaderOptions, Result};

pub(crate) use frontmatter::{parse_metadata_json, parse_metadata_yaml};

/// Parses `CommonMark` text into the document model.
///
/// The strict `CommonMark` preset is the empty extension set; `options.extensions` additionally
/// enables `strikeout`, `subscript`, `superscript`, `hard_line_breaks`, and `task_lists`.
/// `raw_html` is always honored, so toggling it has no effect on the produced document.
#[derive(Debug, Default, Clone, Copy)]
pub struct CommonmarkReader;

impl Reader for CommonmarkReader {
    fn read(&self, input: &str, options: &ReaderOptions) -> Result<Document> {
        let ext = options.extensions;
        let normalized = normalize(input);
        let frontmatter::FrontMatter { meta, body } = frontmatter::extract(&normalized, options)?;
        let source = body.as_deref().unwrap_or(&normalized);
        let (ir, refs, footnotes, examples) = block::parse(source, ext, options.greedy_paragraphs);
        let blocks = resolve::resolve_document(
            &ir,
            refs,
            &footnotes,
            &examples,
            ext,
            options.greedy_paragraphs,
        );
        Ok(Document {
            meta: meta.into_iter().map(|(k, v)| (k.into(), v)).collect(),
            blocks,
            ..Document::default()
        })
    }
}

/// A block whose leaf content is still raw, undifferentiated text awaiting the inline phase.
#[derive(Debug, Clone)]
pub(crate) enum IrBlock {
    /// A paragraph rendered as `Para` (loose context).
    Para(String),
    /// A paragraph rendered as `Plain` (tight list item).
    Plain(String),
    Heading(i32, String),
    CodeBlock(Attr, String),
    RawHtml(String),
    /// A raw block in a named passthrough format (e.g. a fenced ```` ```{=latex} ```` block).
    RawBlock(Format, String),
    ThematicBreak,
    /// A fenced div: its attributes and the recursively-parsed block content.
    Div(Attr, Vec<IrBlock>),
    BlockQuote(Vec<IrBlock>),
    /// A line block: one entry per source line, each still-raw text parsed into inlines in the
    /// inline phase. Division into lines and any preserved leading spaces are already baked into
    /// the strings.
    LineBlock(Vec<String>),
    /// A definition list: one entry per term. Each term's raw text is parsed into inlines in the
    /// inline phase; its definitions are already-resolved block lists with tight-vs-loose paragraph
    /// demotion applied.
    DefinitionList(Vec<IrDefItem>),
    BulletList(Vec<Vec<IrBlock>>),
    OrderedList(ListAttributes, Vec<Vec<IrBlock>>),
    /// A pipe table: per-column alignments, the header row's cell texts, and the body rows' cell
    /// texts. Each cell's text is parsed into inlines in the inline phase. Any caption is attached
    /// after the block phase.
    Table {
        alignments: Vec<Alignment>,
        header: Vec<String>,
        rows: Vec<Vec<String>>,
        caption: Option<String>,
        /// Attributes attached via the caption line when `table_attributes` is enabled.
        attr: Attr,
    },
    /// A grid table: column specs plus header and body rows of still-raw cell text, each cell parsed
    /// as block content in the inline phase. Any caption is attached after the block phase.
    GridTable(Box<grid::GridTable>),
    /// A dash-ruled table: column specs plus an optional header row and body rows of still-raw cell
    /// text, each cell parsed as inline content in the inline phase. Any caption is attached after
    /// the block phase.
    TextTable(Box<texttable::TextTable>),
}

/// One entry of a definition list: a term plus its definitions. The term holds raw text awaiting
/// the inline phase; each definition is its block content (paragraph demotion to `Plain` already
/// applied for tight entries).
#[derive(Debug, Clone)]
pub(crate) struct IrDefItem {
    pub term: String,
    pub definitions: Vec<Vec<IrBlock>>,
}

/// A resolved link reference definition: its destination URL and optional title.
#[derive(Debug, Clone)]
pub(crate) struct LinkDef {
    pub url: String,
    pub title: String,
}

/// Reference definitions keyed by their normalized label: the explicit `[label]: url` definitions,
/// plus the implicit definitions a heading contributes when `implicit_header_references` is on. A
/// heading's label is its source text normalized the same way, so both kinds resolve through one
/// lookup; an explicit definition, registered first, wins over a heading with the same label.
pub(crate) type RefMap = BTreeMap<String, LinkDef>;

/// Footnote definitions, keyed by their normalized label; each value is the still-raw block content
/// gathered for that footnote, resolved into a `Note` at every matching reference.
pub(crate) type FootnoteDefs = BTreeMap<String, Vec<IrBlock>>;

/// Example-list item numbers, keyed by `@label`. The block phase walks every example list in
/// document order, assigning each distinct label the next number in a single shared sequence; a
/// later `@label` reference resolves to that number.
pub(crate) type ExampleMap = BTreeMap<String, i32>;

/// Parse the text of a block-level metadata value into blocks, reusing the full block and inline
/// pipeline. Front matter is not re-extracted, so a metadata value never recurses into another
/// metadata block.
pub(crate) fn parse_meta_blocks(
    text: &str,
    extensions: Extensions,
    greedy_paragraphs: bool,
) -> Vec<Block> {
    let normalized = normalize(text);
    let (ir, refs, footnotes, examples) = block::parse(&normalized, extensions, greedy_paragraphs);
    resolve::resolve_document(
        &ir,
        refs,
        &footnotes,
        &examples,
        extensions,
        greedy_paragraphs,
    )
}

/// Parse the raw text of a table cell into block content, reusing the full block and inline
/// pipeline. A tight cell (one with no internal blank line) demotes its top-level paragraphs to
/// `Plain`; an empty cell carries no blocks.
pub(crate) fn parse_table_cell(
    text: &str,
    tight: bool,
    extensions: Extensions,
    greedy_paragraphs: bool,
) -> Vec<Block> {
    if text.is_empty() {
        return Vec::new();
    }
    let normalized = normalize(text);
    let (mut ir, refs, footnotes, examples) =
        block::parse(&normalized, extensions, greedy_paragraphs);
    if tight {
        postprocess::demote_loose_paragraphs(&mut ir);
    }
    resolve::resolve_document(
        &ir,
        refs,
        &footnotes,
        &examples,
        extensions,
        greedy_paragraphs,
    )
}

const TAB_STOP: usize = 4;

/// Normalize line endings to `\n`, strip a leading UTF-8 BOM, and expand tabs to spaces.
///
/// Tabs are expanded by character column (reset at each line) so the rest of the parser sees only
/// spaces.
fn normalize(input: &str) -> Cow<'_, str> {
    let without_bom = input.strip_prefix('\u{feff}').unwrap_or(input);
    if memchr::memchr2(b'\r', b'\t', without_bom.as_bytes()).is_none() {
        return Cow::Borrowed(without_bom);
    }
    let mut out = String::with_capacity(without_bom.len());
    let mut column = 0;
    let mut chars = without_bom.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
                column = 0;
            }
            '\n' => {
                out.push('\n');
                column = 0;
            }
            '\t' => {
                let width = TAB_STOP - (column % TAB_STOP);
                for _ in 0..width {
                    out.push(' ');
                }
                column += width;
            }
            other => {
                out.push(other);
                column += 1;
            }
        }
    }
    Cow::Owned(out)
}

pub(crate) fn para(inlines: Vec<Inline>) -> Block {
    Block::Para(inlines)
}

pub(crate) fn plain(inlines: Vec<Inline>) -> Block {
    Block::Plain(inlines)
}

#[cfg(test)]
mod reader_tests;
