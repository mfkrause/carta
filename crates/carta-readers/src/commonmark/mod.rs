//! `CommonMark` reader.
//!
//! Parsing follows the spec's two-phase strategy: the block phase ([`block`]) consumes the input
//! line by line into a tree of [`IrBlock`]s whose leaves still hold raw text, collecting link
//! reference definitions; the inline phase ([`inline`]) then parses each leaf's text into inlines.
//! The result is assembled into a [`Document`] (see `docs/plans/slice-1-commonmark-html.md`).

mod block;
mod cursor;
mod html_block;
mod inline;
mod scan;

use std::collections::BTreeMap;

use carta_ast::{Attr, Block, Document, Inline, ListAttributes};
use carta_core::{Extensions, Reader, ReaderOptions, Result};

/// Parses `CommonMark` text into the document model.
///
/// The strict `CommonMark` preset is the empty extension set; `options.extensions` additionally
/// enables `strikeout`, `subscript`, `superscript`, `hard_line_breaks`, and `task_lists`
/// (see `plans/006-commonmark-easy-extensions.md`). `raw_html` is always honored, so toggling it has
/// no effect on the produced document.
#[derive(Debug, Default, Clone, Copy)]
pub struct CommonmarkReader;

impl Reader for CommonmarkReader {
    fn read(&self, input: &str, options: &ReaderOptions) -> Result<Document> {
        Ok(parse(input, options.extensions))
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

fn parse(input: &str, extensions: Extensions) -> Document {
    let normalized = normalize(input);
    let (ir, refs) = block::parse(&normalized);
    let blocks = inline::resolve_blocks(&ir, &refs, extensions);
    Document {
        blocks,
        ..Document::default()
    }
}

/// Width of a tab stop in columns, used when expanding tabs during preprocessing.
const TAB_STOP: usize = 4;

/// Normalize line endings to `\n`, strip a leading UTF-8 BOM, and expand tabs to spaces.
///
/// Tabs are expanded by character column (reset at each line) so the rest of the parser sees only
/// spaces.
fn normalize(input: &str) -> String {
    let without_bom = input.strip_prefix('\u{feff}').unwrap_or(input);
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
    out
}

/// Helper used by the inline phase to wrap parsed inlines back into AST blocks.
pub(crate) fn para(inlines: Vec<Inline>) -> Block {
    Block::Para(inlines)
}

pub(crate) fn plain(inlines: Vec<Inline>) -> Block {
    Block::Plain(inlines)
}

#[cfg(test)]
mod tests {
    use super::CommonmarkReader;
    use carta_ast::Block;
    use carta_core::{Reader, ReaderOptions};

    fn blocks(input: &str) -> Vec<Block> {
        CommonmarkReader
            .read(input, &ReaderOptions::default())
            .expect("reader should not fail")
            .blocks
    }

    #[test]
    fn long_digit_run_is_not_an_ordered_list() {
        // Regression (found by fuzzing): a digit run longer than nine is not an ordered-list
        // marker, and computing its start value must not overflow.
        let input = format!("{}*:*\n", "8".repeat(34));
        assert!(matches!(blocks(&input).as_slice(), [Block::Para(_)]));
    }

    #[test]
    fn ordered_list_start_caps_at_nine_digits() {
        assert!(matches!(
            blocks("999999999. a\n").as_slice(),
            [Block::OrderedList(..)]
        ));
        assert!(matches!(
            blocks("1234567890. a\n").as_slice(),
            [Block::Para(_)]
        ));
    }
}
