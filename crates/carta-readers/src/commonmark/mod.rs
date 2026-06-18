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
    /// A fenced div: its attributes and the recursively-parsed block content.
    Div(Attr, Vec<IrBlock>),
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
    fn bare_marker_trailed_by_spaces_leaves_an_empty_item() {
        // The whitespace after a contentless marker is not a non-blank line, so it leaves the item
        // empty rather than opening an indented code block inside it.
        assert!(matches!(
            blocks("-     \n").as_slice(),
            [Block::BulletList(items)] if items.as_slice() == [Vec::new()]
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

    #[test]
    fn fenced_div_bare_word_names_a_single_class() {
        let result = blocks_with("::: warning\nbody\n:::\n", Extension::FencedDivs);
        let [Block::Div(attr, children)] = result.as_slice() else {
            panic!("expected a single div, got {result:?}");
        };
        assert!(attr.id.is_empty());
        assert_eq!(attr.classes, ["warning"]);
        assert!(attr.attributes.is_empty());
        assert!(matches!(children.as_slice(), [Block::Para(_)]));
    }

    #[test]
    fn fenced_div_brace_spec_carries_id_classes_and_pairs() {
        let result = blocks_with("::: {#a .b .c k=v}\nbody\n:::\n", Extension::FencedDivs);
        let [Block::Div(attr, _)] = result.as_slice() else {
            panic!("expected a single div, got {result:?}");
        };
        assert_eq!(attr.id, "a");
        assert_eq!(attr.classes, ["b", "c"]);
        assert_eq!(
            attr.attributes,
            [("k".to_owned(), "v".to_owned())]
        );
    }

    #[test]
    fn fenced_divs_nest_with_the_inner_closing_first() {
        let result =
            blocks_with("::: outer\n::: inner\nx\n:::\ny\n:::\n", Extension::FencedDivs);
        let [Block::Div(outer, outer_children)] = result.as_slice() else {
            panic!("expected a single outer div, got {result:?}");
        };
        assert_eq!(outer.classes, ["outer"]);
        let [Block::Div(inner, _), Block::Para(_)] = outer_children.as_slice() else {
            panic!("outer should hold an inner div then a paragraph, got {outer_children:?}");
        };
        assert_eq!(inner.classes, ["inner"]);
    }

    #[test]
    fn a_shorter_colon_run_does_not_close_a_longer_fence() {
        // The div opens with four colons, so a three-colon line inside it is ordinary text and the
        // div runs to the matching four-colon close.
        let result =
            blocks_with(":::: wide\n:::\nstill inside\n::::\n", Extension::FencedDivs);
        let [Block::Div(attr, children)] = result.as_slice() else {
            panic!("expected a single div, got {result:?}");
        };
        assert_eq!(attr.classes, ["wide"]);
        assert!(matches!(children.as_slice(), [Block::Para(_)]));
    }

    #[test]
    fn fenced_div_syntax_without_the_extension_stays_text() {
        // With the toggle off, the colon fences are ordinary paragraph text and no div is produced.
        let result = blocks("::: warning\nbody\n:::\n");
        assert!(result.iter().all(|b| !matches!(b, Block::Div(..))));
    }

    #[test]
    fn blank_after_a_div_in_a_list_item_makes_the_list_loose() {
        let result =
            blocks_with("- ::: note\n  inside\n  :::\n\n  after\n", Extension::FencedDivs);
        // The blank between the closed div and `after` is a gap inside the item, so the list is
        // loose and the trailing paragraph stays `Para` rather than being demoted to `Plain`.
        let [Block::BulletList(items)] = result.as_slice() else {
            panic!("expected a single bullet list, got {result:?}");
        };
        let Some([Block::Div(..), tail]) = items.first().map(Vec::as_slice) else {
            panic!("the item should hold a div then a trailing block, got {items:?}");
        };
        assert!(
            matches!(tail, Block::Para(_)),
            "loose list should keep the trailing paragraph as Para, got {tail:?}"
        );
    }

    #[test]
    fn blank_ending_a_nested_block_quote_makes_the_list_loose() {
        // The blank line after the first item's block quote leaves that quote unmatched, so it
        // ends there and the blank counts toward the list's looseness. A loose list keeps its item
        // paragraphs as `Para` (a tight list would demote them to `Plain`).
        let result = blocks("- item\n  > q\n\n- item2\n");
        let [Block::BulletList(items)] = result.as_slice() else {
            panic!("expected a single bullet list, got {result:?}");
        };
        let Some([first, ..]) = items.first().map(Vec::as_slice) else {
            panic!("the first item should have content");
        };
        assert!(
            matches!(first, Block::Para(_)),
            "loose list should keep the item paragraph as Para, got {first:?}"
        );
    }

    #[test]
    fn image_only_paragraph_becomes_a_figure_captioned_by_its_alt_text() {
        let result = blocks_with("![a gull](gull.png)\n", Extension::ImplicitFigures);
        let [Block::Figure(attr, caption, body)] = result.as_slice() else {
            panic!("expected a single figure, got {result:?}");
        };
        assert_eq!(*attr, carta_ast::Attr::default());
        assert!(caption.short.is_none());
        // The caption is a clone of the image's alt inlines wrapped in one `Plain`.
        let [Block::Plain(caption_inlines)] = caption.long.as_slice() else {
            panic!("caption should be a single Plain, got {:?}", caption.long);
        };
        assert!(matches!(
            caption_inlines.as_slice(),
            [Inline::Str(a), Inline::Space, Inline::Str(b)] if a == "a" && b == "gull"
        ));
        // The body is the original image, unchanged, inside a single `Plain`.
        let [Block::Plain(image_inlines)] = body.as_slice() else {
            panic!("body should be a single Plain, got {body:?}");
        };
        let [Inline::Image(_, alt, target)] = image_inlines.as_slice() else {
            panic!("body should wrap an Image, got {image_inlines:?}");
        };
        assert_eq!(*caption_inlines, *alt, "alt is duplicated into the caption");
        assert_eq!(target.url, "gull.png");
    }

    #[test]
    fn an_empty_alt_image_stays_a_paragraph() {
        // The decisive condition is a non-empty alt; a title does not change that.
        let result = blocks_with("![](spacer.png \"t\")\n", Extension::ImplicitFigures);
        let [Block::Para(inlines)] = result.as_slice() else {
            panic!("expected a paragraph, got {result:?}");
        };
        assert!(matches!(inlines.as_slice(), [Inline::Image(_, alt, _)] if alt.is_empty()));
    }

    #[test]
    fn the_image_title_is_not_used_as_the_caption() {
        let result = blocks_with("![cap](c.png \"tooltip\")\n", Extension::ImplicitFigures);
        let [Block::Figure(_, caption, _)] = result.as_slice() else {
            panic!("expected a figure, got {result:?}");
        };
        let [Block::Plain(inlines)] = caption.long.as_slice() else {
            panic!("caption should be a single Plain, got {:?}", caption.long);
        };
        assert!(matches!(inlines.as_slice(), [Inline::Str(s)] if s == "cap"));
    }

    #[test]
    fn an_extra_inline_or_a_wrapper_keeps_the_paragraph() {
        // A second inline disqualifies the paragraph.
        assert!(matches!(
            blocks_with("look at ![this](i.png)\n", Extension::ImplicitFigures).as_slice(),
            [Block::Para(_)]
        ));
        // A link wrapping the image makes the link the sole inline, not the image.
        let linked = blocks_with("[![a](i.png)](u)\n", Extension::ImplicitFigures);
        let [Block::Para(inlines)] = linked.as_slice() else {
            panic!("expected a paragraph, got {linked:?}");
        };
        assert!(matches!(inlines.as_slice(), [Inline::Link(..)]));
    }

    #[test]
    fn implicit_figures_off_keeps_the_image_paragraph() {
        assert!(matches!(
            blocks("![a gull](gull.png)\n").as_slice(),
            [Block::Para(_)]
        ));
    }
}
