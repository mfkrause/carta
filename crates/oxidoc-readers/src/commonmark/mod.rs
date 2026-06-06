//! `CommonMark` reader.
//!
//! Parsing follows the spec's two-phase strategy: the block phase ([`block`]) consumes the input
//! line by line into a tree of [`IrBlock`]s whose leaves still hold raw text, collecting link
//! reference definitions; the inline phase ([`inline`]) then parses each leaf's text into inlines.
//! The result is assembled into a [`Document`]. The target output is `pandoc -f commonmark`'s JSON
//! AST, verified differentially against the pinned binary (see `docs/plans/slice-1-commonmark-html.md`).

mod block;
mod inline;

use std::collections::BTreeMap;

use oxidoc_ast::{Attr, Block, Document, Inline, ListAttributes};
use oxidoc_core::{Reader, ReaderOptions, Result};

/// Parses `CommonMark` text into the document model.
#[derive(Debug, Default, Clone, Copy)]
pub struct CommonmarkReader;

impl Reader for CommonmarkReader {
    fn read(&self, input: &str, _options: &ReaderOptions) -> Result<Document> {
        Ok(parse(input))
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
    ThematicBreak,
    BlockQuote(Vec<IrBlock>),
    BulletList(Vec<Vec<IrBlock>>),
    OrderedList(ListAttributes, Vec<Vec<IrBlock>>),
}

/// A resolved link reference definition: its destination URL and optional title.
#[derive(Debug, Clone)]
pub(crate) struct LinkDef {
    pub url: String,
    pub title: String,
}

/// Link reference definitions, keyed by their normalized label.
pub(crate) type RefMap = BTreeMap<String, LinkDef>;

fn parse(input: &str) -> Document {
    let normalized = normalize(input);
    let (ir, refs) = block::parse(&normalized);
    let blocks = inline::resolve_blocks(&ir, &refs);
    Document {
        blocks,
        ..Document::default()
    }
}

/// Normalize line endings to `\n` and strip a leading UTF-8 BOM, per the spec's preprocessing.
fn normalize(input: &str) -> String {
    let without_bom = input.strip_prefix('\u{feff}').unwrap_or(input);
    let mut out = String::with_capacity(without_bom.len());
    let mut chars = without_bom.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            other => out.push(other),
        }
    }
    out
}

/// Helper used by the inline phase to wrap parsed inlines back into AST blocks.
pub(crate) fn para(inlines: Vec<Inline>) -> Block {
    Block::Para(inlines)
}

pub(crate) fn plain(inlines: Vec<Inline>) -> Block {
    Block::Plain(inlines)
}
