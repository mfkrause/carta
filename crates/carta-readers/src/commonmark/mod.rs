//! `CommonMark` reader.
//!
//! Parsing follows the spec's two-phase strategy: the block phase ([`block`]) consumes the input
//! line by line into a tree of [`IrBlock`]s whose leaves still hold raw text, collecting link
//! reference definitions; the inline phase ([`inline`]) then parses each leaf's text into inlines.
//! The result is assembled into a [`Document`] (see `docs/plans/slice-1-commonmark-html.md`).

mod attr;
mod autolink;
mod block;
mod cursor;
mod html_block;
mod inline;
mod scan;
mod table;

use std::collections::BTreeMap;

use carta_ast::{Alignment, Attr, Block, Document, Inline, ListAttributes};
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
    /// A pipe table: per-column alignments, the header row's cell texts, and the body rows' cell
    /// texts. Each cell's text is parsed into inlines in the inline phase.
    Table {
        alignments: Vec<Alignment>,
        header: Vec<String>,
        rows: Vec<Vec<String>>,
    },
}

/// A resolved link reference definition: its destination URL and optional title.
#[derive(Debug, Clone)]
pub(crate) struct LinkDef {
    pub url: String,
    pub title: String,
}

/// Link reference definitions, keyed by their normalized label.
pub(crate) type RefMap = BTreeMap<String, LinkDef>;

/// Footnote definitions, keyed by their normalized label; each value is the still-raw block content
/// gathered for that footnote, resolved into a `Note` at every matching reference.
pub(crate) type FootnoteDefs = BTreeMap<String, Vec<IrBlock>>;

fn parse(input: &str, extensions: Extensions) -> Document {
    let normalized = normalize(input);
    let (ir, refs, footnotes) = block::parse(&normalized, extensions);
    let blocks = inline::resolve_document(&ir, &refs, &footnotes, extensions);
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
    use carta_ast::{Block, Inline};
    use carta_core::{Extension, Extensions, Reader, ReaderOptions};

    fn blocks(input: &str) -> Vec<Block> {
        CommonmarkReader
            .read(input, &ReaderOptions::default())
            .expect("reader should not fail")
            .blocks
    }

    fn blocks_with(input: &str, ext: Extension) -> Vec<Block> {
        let mut extensions = Extensions::empty();
        extensions.insert(ext);
        let mut options = ReaderOptions::default();
        options.extensions = extensions;
        CommonmarkReader
            .read(input, &options)
            .expect("reader should not fail")
            .blocks
    }

    /// The inlines of a single-paragraph document, for footnote assertions.
    fn para_inlines(input: &str, ext: Extension) -> Vec<Inline> {
        match blocks_with(input, ext).as_slice() {
            [Block::Para(inlines)] => inlines.clone(),
            other => panic!("expected a single paragraph, got {other:?}"),
        }
    }

    #[test]
    fn footnote_reference_resolves_to_a_note_and_lifts_the_definition() {
        // The definition leaves the body, so only the referencing paragraph remains, and its
        // reference becomes a note carrying the definition's blocks.
        let inlines = para_inlines("text[^a]\n\n[^a]: body\n", Extension::Footnotes);
        let note = inlines
            .iter()
            .find_map(|inline| match inline {
                Inline::Note(blocks) => Some(blocks.clone()),
                _ => None,
            })
            .expect("a note should be present");
        assert!(matches!(note.as_slice(), [Block::Para(_)]));
    }

    #[test]
    fn undefined_footnote_reference_stays_literal() {
        // With no matching definition the brackets are ordinary text and no note is produced.
        let inlines = para_inlines("text[^missing]\n", Extension::Footnotes);
        assert!(inlines.iter().all(|i| !matches!(i, Inline::Note(_))));
        assert!(
            inlines
                .iter()
                .any(|i| matches!(i, Inline::Str(s) if s.contains("[^missing]")))
        );
    }

    #[test]
    fn footnote_extension_off_produces_no_note() {
        // Without the toggle `[^a]: body` is an ordinary link reference definition, so `[^a]`
        // resolves to a link and no note is created.
        let result = blocks("text[^a]\n\n[^a]: body\n");
        let [Block::Para(inlines)] = result.as_slice() else {
            panic!("expected a single paragraph, got {result:?}");
        };
        assert!(inlines.iter().any(|i| matches!(i, Inline::Link(..))));
        assert!(inlines.iter().all(|i| !matches!(i, Inline::Note(_))));
    }

    #[test]
    fn footnote_definition_spans_indented_continuation_blocks() {
        let inlines = para_inlines(
            "ref[^a]\n\n[^a]: first\n\n    second\n",
            Extension::Footnotes,
        );
        let note = inlines
            .iter()
            .find_map(|inline| match inline {
                Inline::Note(blocks) => Some(blocks.clone()),
                _ => None,
            })
            .expect("a note should be present");
        assert!(matches!(note.as_slice(), [Block::Para(_), Block::Para(_)]));
    }

    #[test]
    fn nested_footnote_reference_inside_a_definition_does_not_nest() {
        // A reference within a definition's own body collapses to an empty string rather than
        // embedding a further note.
        let inlines = para_inlines(
            "ref[^a]\n\n[^a]: see [^b]\n\n[^b]: inner\n",
            Extension::Footnotes,
        );
        let note = inlines
            .iter()
            .find_map(|inline| match inline {
                Inline::Note(blocks) => Some(blocks.clone()),
                _ => None,
            })
            .expect("a note should be present");
        let Some(Block::Para(body)) = note.first() else {
            panic!("note should hold a paragraph");
        };
        assert!(body.iter().all(|i| !matches!(i, Inline::Note(_))));
    }

    #[test]
    fn footnote_labels_fold_case_and_whitespace() {
        let inlines = para_inlines("ref[^A B]\n\n[^a   b]: body\n", Extension::Footnotes);
        assert!(inlines.iter().any(|i| matches!(i, Inline::Note(_))));
    }

    #[test]
    fn defined_footnote_reference_wins_over_a_following_inline_target() {
        // A defined reference consumes nothing past `]`, so the `(url)` stays literal text.
        let inlines = para_inlines("[^a](url)\n\n[^a]: body\n", Extension::Footnotes);
        assert!(inlines.iter().any(|i| matches!(i, Inline::Note(_))));
        assert!(
            inlines
                .iter()
                .any(|i| matches!(i, Inline::Str(s) if s.contains("(url)")))
        );
    }

    #[test]
    fn empty_list_marker_below_an_unmatched_container_starts_a_list() {
        // The paragraph that the `- ` could interrupt sits in the unmatched block quote, a level
        // below where the marker opens, so the marker is not interrupting it: the quote closes and
        // an empty bullet list begins rather than the `- ` continuing the paragraph lazily.
        let result = blocks("> two\n- \n");
        assert!(matches!(
            result.as_slice(),
            [Block::BlockQuote(_), Block::BulletList(items)] if items.as_slice() == [Vec::new()]
        ));
    }

    #[test]
    fn empty_list_marker_still_cannot_interrupt_a_same_level_paragraph() {
        // At the same level the restriction holds: an empty marker is absorbed into the paragraph.
        // (`*` is used rather than `-` so the line is not read as a setext heading underline.)
        assert!(matches!(blocks("para\n* \n").as_slice(), [Block::Para(_)]));
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
