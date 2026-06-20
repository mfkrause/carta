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
mod frontmatter;
mod grid;
mod html_block;
mod identifiers;
mod inline;
mod scan;
mod table;
mod texttable;
mod yaml;

use std::collections::BTreeMap;

use carta_ast::{Alignment, Attr, Block, Document, Format, Inline, ListAttributes};
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
        let ext = options.extensions;
        let normalized = normalize(input);
        let frontmatter::FrontMatter { meta, body } = frontmatter::extract(&normalized, options)?;
        let source = body.as_deref().unwrap_or(&normalized);
        let (ir, refs, footnotes, examples) = block::parse(source, ext, options.greedy_paragraphs);
        let blocks = inline::resolve_document(
            &ir,
            refs,
            &footnotes,
            &examples,
            ext,
            options.greedy_paragraphs,
        );
        Ok(Document {
            meta,
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
    inline::resolve_document(
        &ir,
        refs,
        &footnotes,
        &examples,
        extensions,
        greedy_paragraphs,
    )
}

/// Parse the raw text of a table cell into block content, reusing the full block and inline
/// pipeline. A tight cell — one with no internal blank line — demotes its top-level paragraphs to
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
        block::demote_loose_paragraphs(&mut ir);
    }
    inline::resolve_document(
        &ir,
        refs,
        &footnotes,
        &examples,
        extensions,
        greedy_paragraphs,
    )
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
    use carta_ast::{Attr, Block, Document, Inline, ListNumberDelim, ListNumberStyle, Target};
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

    fn blocks_with_many(input: &str, exts: &[Extension]) -> Vec<Block> {
        let mut extensions = Extensions::empty();
        for ext in exts {
            extensions.insert(*ext);
        }
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

    /// Read in the markdown dialect (greedy paragraphs) with the given extensions enabled.
    fn read_markdown(input: &str, exts: &[Extension]) -> Document {
        let mut extensions = Extensions::empty();
        for ext in exts {
            extensions.insert(*ext);
        }
        let mut options = ReaderOptions::default();
        options.extensions = extensions;
        options.greedy_paragraphs = true;
        CommonmarkReader
            .read(input, &options)
            .expect("reader should not fail")
    }

    #[test]
    fn grid_cell_inlines_honor_the_markdown_dialect() {
        // A grid-table cell parses its content under the document's dialect: in the markdown dialect
        // a superscript rejects an inner space, so `^a b^` stays literal rather than wrapping.
        let input = "+-------+\n| ^a b^ |\n+-------+\n";
        let doc = read_markdown(input, &[Extension::GridTables, Extension::Superscript]);
        let table = match doc.blocks.as_slice() {
            [Block::Table(table)] => table,
            other => panic!("expected a single table, got {other:?}"),
        };
        let cell = table
            .bodies
            .first()
            .and_then(|body| body.body.first())
            .and_then(|row| row.cells.first())
            .expect("a single body cell");
        let inlines = match cell.content.as_slice() {
            [Block::Plain(inlines)] => inlines,
            other => panic!("expected a plain cell, got {other:?}"),
        };
        assert!(
            inlines.iter().all(|i| !matches!(i, Inline::Superscript(_))),
            "grid cell should not build a superscript around an inner space: {inlines:?}"
        );
    }

    #[test]
    fn metadata_values_honor_the_markdown_dialect() {
        use carta_ast::MetaValue;
        // A YAML metadata value parses under the document's dialect too: the superscript with an
        // inner space stays literal and the code span trims its padding to `x`.
        let input = "---\ntitle: ^a b^ `  x  `\n---\n\nbody\n";
        let doc = read_markdown(
            input,
            &[Extension::YamlMetadataBlock, Extension::Superscript],
        );
        let inlines = match doc.meta.get("title") {
            Some(MetaValue::MetaInlines(inlines)) => inlines,
            other => panic!("expected inline metadata, got {other:?}"),
        };
        assert!(
            inlines.iter().all(|i| !matches!(i, Inline::Superscript(_))),
            "metadata should not build a superscript around an inner space: {inlines:?}"
        );
        assert!(
            inlines
                .iter()
                .any(|i| matches!(i, Inline::Code(_, code) if code == "x")),
            "metadata code span should trim to `x`: {inlines:?}"
        );
    }

    #[test]
    fn attribute_only_table_caption_carries_no_blocks() {
        // A caption line that is nothing but a trailing attribute block: the block is split off onto
        // the table's own attributes, leaving the caption text empty. An empty caption parses to no
        // blocks at all, never a `Plain` wrapping an empty inline list.
        let input = "| a | b |\n|---|---|\n| 1 | 2 |\n\n: {#tid}\n";
        let blocks = blocks_with_many(
            input,
            &[
                Extension::PipeTables,
                Extension::TableCaptions,
                Extension::TableAttributes,
            ],
        );
        let table = match blocks.as_slice() {
            [Block::Table(table)] => table,
            other => panic!("expected a single table, got {other:?}"),
        };
        assert!(table.caption.long.is_empty());
        assert_eq!(table.attr.id, "tid");
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
        assert_eq!(attr.attributes, [("k".to_owned(), "v".to_owned())]);
    }

    #[test]
    fn fenced_divs_nest_with_the_inner_closing_first() {
        let result = blocks_with(
            "::: outer\n::: inner\nx\n:::\ny\n:::\n",
            Extension::FencedDivs,
        );
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
        let result = blocks_with(
            ":::: wide\n:::\nstill inside\n::::\n",
            Extension::FencedDivs,
        );
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
        let result = blocks_with(
            "- ::: note\n  inside\n  :::\n\n  after\n",
            Extension::FencedDivs,
        );
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

    fn header_ids(blocks: &[Block]) -> Vec<String> {
        blocks
            .iter()
            .filter_map(|b| match b {
                Block::Header(_, attr, _) => Some(attr.id.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn gfm_auto_identifiers_slug_headers_and_count_duplicates() {
        let result = blocks_with(
            "# Foo & Bar\n\n# 1.2 Section\n\n# Foo & Bar\n",
            Extension::GfmAutoIdentifiers,
        );
        // Punctuation drops without collapsing the gaps, dots vanish, leading digits stay, and a
        // repeated slug is suffixed by its occurrence count.
        assert_eq!(
            header_ids(&result),
            ["foo--bar", "12-section", "foo--bar-1"]
        );
    }

    #[test]
    fn auto_identifiers_strip_leading_runs_and_increment_until_unique() {
        let result = blocks_with(
            "# 1. Intro\n\n# Intro\n\n# Intro\n",
            Extension::AutoIdentifiers,
        );
        // The leading non-letter run is stripped, then each repeat increments until the whole
        // identifier is unused.
        assert_eq!(header_ids(&result), ["intro", "intro-1", "intro-2"]);
    }

    #[test]
    fn auto_identifiers_fall_back_to_section_for_empty_slugs() {
        let result = blocks_with("# !!!\n\n# ???\n", Extension::AutoIdentifiers);
        // Both headings reduce to nothing, so the fallback `section` applies and the second is
        // disambiguated.
        assert_eq!(header_ids(&result), ["section", "section-1"]);
    }

    #[test]
    fn auto_identifiers_off_leaves_headers_unidentified() {
        assert_eq!(header_ids(&blocks("# Hello World\n")), [""]);
    }

    const HEADER_REFS: &[Extension] = &[
        Extension::GfmAutoIdentifiers,
        Extension::ImplicitHeaderReferences,
    ];

    /// The link and image targets reached from every paragraph, in order.
    fn reference_targets(blocks: &[Block]) -> Vec<String> {
        fn collect(inlines: &[Inline], out: &mut Vec<String>) {
            for inline in inlines {
                match inline {
                    Inline::Link(_, _, target) | Inline::Image(_, _, target) => {
                        out.push(target.url.clone());
                    }
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        for block in blocks {
            if let Block::Para(inlines) = block {
                collect(inlines, &mut out);
            }
        }
        out
    }

    #[test]
    fn implicit_header_references_resolve_a_shortcut_reference() {
        let result = blocks_with_many("# Some Heading\n\n[Some Heading]\n", HEADER_REFS);
        // The heading registers a definition keyed by its label, so the bare reference links to
        // the heading's identifier.
        assert_eq!(reference_targets(&result), ["#some-heading"]);
    }

    #[test]
    fn implicit_header_references_match_full_collapsed_and_image_forms() {
        let result = blocks_with_many(
            "# Some Heading\n\n[text][Some Heading] [Some Heading][] ![Some Heading]\n",
            HEADER_REFS,
        );
        // Full, collapsed, and image references all resolve to the same anchor.
        assert_eq!(
            reference_targets(&result),
            ["#some-heading", "#some-heading", "#some-heading"]
        );
    }

    #[test]
    fn implicit_header_references_fold_case_and_collapse_whitespace() {
        let result = blocks_with_many("# Some Heading\n\n[SOME    HEADING]\n", HEADER_REFS);
        assert_eq!(reference_targets(&result), ["#some-heading"]);
    }

    #[test]
    fn implicit_header_references_match_on_label_source_not_decoded_text() {
        // The label is matched against the heading's literal source, so the marked-up form
        // resolves while the same words without the emphasis markers do not.
        let result = blocks_with_many(
            "# Heading with *emphasis*\n\n[Heading with *emphasis*] [Heading with emphasis]\n",
            HEADER_REFS,
        );
        assert_eq!(reference_targets(&result), ["#heading-with-emphasis"]);
    }

    #[test]
    fn an_explicit_definition_outranks_an_implicit_header_reference() {
        let result = blocks_with_many(
            "# Linked Elsewhere\n\n[Linked Elsewhere]: https://example.com/x\n\n[Linked Elsewhere]\n",
            HEADER_REFS,
        );
        // An explicit definition with the same label is registered first and keeps the link.
        assert_eq!(reference_targets(&result), ["https://example.com/x"]);
    }

    #[test]
    fn a_repeated_heading_is_reachable_only_through_the_first() {
        let result = blocks_with_many("# Twice\n\n# Twice\n\n[Twice]\n", HEADER_REFS);
        // The first heading keeps the bare identifier; the reference resolves to it, not the
        // disambiguated second occurrence.
        assert_eq!(reference_targets(&result), ["#twice"]);
    }

    #[test]
    fn implicit_header_references_resolve_before_their_heading() {
        let result = blocks_with_many("[Later Section]\n\n# Later Section\n", HEADER_REFS);
        // A reference may precede the heading it points at.
        assert_eq!(reference_targets(&result), ["#later-section"]);
    }

    #[test]
    fn implicit_header_references_off_leaves_the_label_literal() {
        let result = blocks_with(
            "# Some Heading\n\n[Some Heading]\n",
            Extension::GfmAutoIdentifiers,
        );
        assert!(reference_targets(&result).is_empty());
        let [_, Block::Para(inlines)] = result.as_slice() else {
            panic!("expected a heading then a paragraph, got {result:?}");
        };
        assert!(
            inlines
                .iter()
                .any(|i| matches!(i, Inline::Str(s) if s.contains("[Some")))
        );
    }

    const LINE_BLOCKS: &[Extension] = &[Extension::LineBlocks];
    const LINE_BLOCKS_TABLES: &[Extension] = &[Extension::LineBlocks, Extension::PipeTables];

    /// Plain-text rendering of one inline run, enough to assert a line block's entries.
    fn flatten_inlines(inlines: &[Inline]) -> String {
        let mut out = String::new();
        for inline in inlines {
            match inline {
                Inline::Str(text) | Inline::Code(_, text) => out.push_str(text),
                Inline::Space | Inline::SoftBreak | Inline::LineBreak => out.push(' '),
                Inline::Emph(children)
                | Inline::Strong(children)
                | Inline::Link(_, children, _) => out.push_str(&flatten_inlines(children)),
                _ => {}
            }
        }
        out
    }

    /// The flattened text of every entry across all line blocks in a document.
    fn line_block_entries(blocks: &[Block]) -> Vec<String> {
        let mut entries = Vec::new();
        for block in blocks {
            if let Block::LineBlock(lines) = block {
                entries.extend(lines.iter().map(|line| flatten_inlines(line)));
            }
        }
        entries
    }

    #[test]
    fn line_block_keeps_each_marked_line_as_its_own_entry() {
        let blocks = blocks_with_many("| Line one\n| Line two\n", LINE_BLOCKS);
        assert!(matches!(blocks.as_slice(), [Block::LineBlock(_)]));
        assert_eq!(line_block_entries(&blocks), ["Line one", "Line two"]);
    }

    #[test]
    fn line_block_preserves_leading_spaces_as_non_breaking() {
        let blocks = blocks_with_many("|   indented\n", LINE_BLOCKS);
        assert_eq!(line_block_entries(&blocks), ["\u{a0}\u{a0}indented"]);
    }

    #[test]
    fn line_block_bar_alone_is_an_empty_entry() {
        let blocks = blocks_with_many("|\n| after\n", LINE_BLOCKS);
        assert_eq!(line_block_entries(&blocks), ["", "after"]);
    }

    #[test]
    fn line_block_folds_an_indented_continuation_into_the_entry_above() {
        let blocks = blocks_with_many("| first part\n  second part\n", LINE_BLOCKS);
        assert_eq!(line_block_entries(&blocks), ["first part second part"]);
    }

    #[test]
    fn line_block_collapses_internal_runs_and_drops_trailing_space() {
        let blocks = blocks_with_many("| a    b    c   \n", LINE_BLOCKS);
        assert_eq!(line_block_entries(&blocks), ["a b c"]);
    }

    #[test]
    fn line_block_all_space_entry_collapses_to_empty() {
        let blocks = blocks_with_many("|    \n| x\n", LINE_BLOCKS);
        assert_eq!(line_block_entries(&blocks), ["", "x"]);
    }

    #[test]
    fn a_bar_without_a_following_space_is_not_a_line_block() {
        let blocks = blocks_with_many("|nospace\n", LINE_BLOCKS);
        assert!(matches!(blocks.as_slice(), [Block::Para(_)]));
    }

    #[test]
    fn a_line_block_does_not_interrupt_a_paragraph() {
        let blocks = blocks_with_many("ordinary text\n| still the paragraph\n", LINE_BLOCKS);
        assert!(matches!(blocks.as_slice(), [Block::Para(_)]));
        assert!(line_block_entries(&blocks).is_empty());
    }

    #[test]
    fn a_blank_line_ends_a_line_block() {
        let blocks = blocks_with_many("| a\n\nplain\n", LINE_BLOCKS);
        assert!(matches!(
            blocks.as_slice(),
            [Block::LineBlock(_), Block::Para(_)]
        ));
    }

    #[test]
    fn a_whitespace_only_line_continues_a_non_empty_entry() {
        // Unlike a wholly blank line, a line of only spaces folds into the entry above it (adding
        // nothing), so the block stays open and the next bar line is a second entry.
        let blocks = blocks_with_many("| a\n  \n| b\n", LINE_BLOCKS);
        assert!(matches!(blocks.as_slice(), [Block::LineBlock(_)]));
        assert_eq!(line_block_entries(&blocks), ["a", "b"]);
    }

    #[test]
    fn a_continuation_under_an_empty_entry_ends_the_block() {
        // With no content to extend, a whitespace-led line closes the block and is reparsed.
        let blocks = blocks_with_many("| \n |\n", LINE_BLOCKS);
        assert!(matches!(
            blocks.as_slice(),
            [Block::LineBlock(_), Block::Para(_)]
        ));
        assert_eq!(line_block_entries(&blocks), [""]);
    }

    #[test]
    fn a_delimiter_row_under_a_single_bar_line_makes_a_table() {
        let blocks = blocks_with_many("| a | b |\n|---|---|\n| 1 | 2 |\n", LINE_BLOCKS_TABLES);
        assert!(matches!(blocks.as_slice(), [Block::Table(_)]));
        assert!(line_block_entries(&blocks).is_empty());
    }

    #[test]
    fn a_bar_line_with_no_delimiter_stays_a_line_block() {
        let blocks = blocks_with_many("| a | b |\nplain\n", LINE_BLOCKS_TABLES);
        assert!(matches!(
            blocks.as_slice(),
            [Block::LineBlock(_), Block::Para(_)]
        ));
    }

    #[test]
    fn with_the_extension_off_a_bar_line_is_literal_paragraph_text() {
        let blocks = blocks("| a\n");
        let [Block::Para(inlines)] = blocks.as_slice() else {
            panic!("expected a single paragraph, got {blocks:?}");
        };
        assert!(matches!(inlines.first(), Some(Inline::Str(text)) if text == "|"));
    }

    /// The (term-text, definitions) pairs of the first definition list in a document.
    fn definition_items(blocks: &[Block]) -> Vec<(String, Vec<Vec<Block>>)> {
        for block in blocks {
            if let Block::DefinitionList(items) = block {
                return items
                    .iter()
                    .map(|(term, defs)| (flatten_inlines(term), defs.clone()))
                    .collect();
            }
        }
        Vec::new()
    }

    #[test]
    fn a_term_above_a_colon_line_becomes_one_tight_definition() {
        let items = definition_items(&blocks_with("apple\n: red\n", Extension::DefinitionLists));
        let [(term, defs)] = items.as_slice() else {
            panic!("expected one item, got {items:?}");
        };
        assert_eq!(term, "apple");
        assert!(matches!(defs.as_slice(), [one] if matches!(one.as_slice(), [Block::Plain(_)])));
    }

    #[test]
    fn a_term_carries_several_definitions_under_colon_or_tilde_markers() {
        let items = definition_items(&blocks_with(
            "water\n: clear\n~ vital\n",
            Extension::DefinitionLists,
        ));
        let [(term, defs)] = items.as_slice() else {
            panic!("expected one item, got {items:?}");
        };
        assert_eq!(term, "water");
        assert_eq!(defs.len(), 2);
    }

    #[test]
    fn consecutive_terms_join_one_list() {
        let items = definition_items(&blocks_with(
            "a\n: x\n\nb\n: y\n",
            Extension::DefinitionLists,
        ));
        let terms: Vec<&str> = items.iter().map(|(term, _)| term.as_str()).collect();
        assert_eq!(terms, ["a", "b"]);
    }

    #[test]
    fn a_blank_line_before_the_marker_makes_the_definition_loose() {
        let items = definition_items(&blocks_with(
            "planet\n\n: orbits\n",
            Extension::DefinitionLists,
        ));
        let [(_, defs)] = items.as_slice() else {
            panic!("expected one item, got {items:?}");
        };
        assert!(matches!(defs.as_slice(), [one] if matches!(one.as_slice(), [Block::Para(_)])));
    }

    #[test]
    fn an_indented_continuation_keeps_a_second_block_in_the_definition() {
        let items = definition_items(&blocks_with(
            "essay\n: first.\n\n  second.\n",
            Extension::DefinitionLists,
        ));
        let [(_, defs)] = items.as_slice() else {
            panic!("expected one item, got {items:?}");
        };
        let [blocks] = defs.as_slice() else {
            panic!("expected one definition, got {defs:?}");
        };
        assert_eq!(blocks.len(), 2);
    }

    #[test]
    fn a_definition_holds_a_nested_block_when_indented_to_the_content_column() {
        let items = definition_items(&blocks_with(
            "shapes\n: items:\n\n    - circle\n    - square\n",
            Extension::DefinitionLists,
        ));
        let [(_, defs)] = items.as_slice() else {
            panic!("expected one item, got {items:?}");
        };
        let [blocks] = defs.as_slice() else {
            panic!("expected one definition, got {defs:?}");
        };
        assert!(matches!(
            blocks.as_slice(),
            [Block::Plain(_), Block::BulletList(_)]
        ));
    }

    #[test]
    fn lines_above_the_marker_fold_into_one_term() {
        let items = definition_items(&blocks_with(
            "one\ntwo\n: both\n",
            Extension::DefinitionLists,
        ));
        let [(term, _)] = items.as_slice() else {
            panic!("expected one item, got {items:?}");
        };
        assert_eq!(term, "one two");
    }

    #[test]
    fn an_unindented_line_lazily_continues_the_definition() {
        let items = definition_items(&blocks_with(
            "apple\n: red\norange\n",
            Extension::DefinitionLists,
        ));
        let [(_, defs)] = items.as_slice() else {
            panic!("expected one item, got {items:?}");
        };
        let [blocks] = defs.as_slice() else {
            panic!("expected one definition, got {defs:?}");
        };
        assert!(matches!(blocks.as_slice(), [Block::Plain(_)]));
    }

    #[test]
    fn a_colon_without_a_following_space_is_not_a_marker() {
        let blocks = blocks_with("term\n:def\n", Extension::DefinitionLists);
        assert!(matches!(blocks.as_slice(), [Block::Para(_)]));
    }

    #[test]
    fn an_empty_definition_yields_an_empty_block_list() {
        let blocks = blocks_with("T\n:\nmore\n", Extension::DefinitionLists);
        let items = definition_items(&blocks);
        let [(term, defs)] = items.as_slice() else {
            panic!("expected one item, got {items:?}");
        };
        assert_eq!(term, "T");
        assert!(matches!(defs.as_slice(), [one] if one.is_empty()));
        // The unindented line ends the list and stands as its own paragraph.
        assert!(matches!(
            blocks.as_slice(),
            [Block::DefinitionList(_), Block::Para(_)]
        ));
    }

    #[test]
    fn an_empty_definition_absorbs_a_deferred_indented_block() {
        // A blank line does not close an as-yet-empty definition; the indented line that follows
        // becomes its body.
        let items = definition_items(&blocks_with(
            "T\n:\n\n    code\n",
            Extension::DefinitionLists,
        ));
        let [(_, defs)] = items.as_slice() else {
            panic!("expected one item, got {items:?}");
        };
        assert!(matches!(defs.as_slice(), [one] if matches!(one.as_slice(), [Block::Plain(_)])));
    }

    #[test]
    fn with_the_extension_off_a_colon_line_is_literal_paragraph_text() {
        let blocks = blocks("apple\n: red\n");
        assert!(matches!(blocks.as_slice(), [Block::Para(_)]));
        assert!(definition_items(&blocks).is_empty());
    }

    /// Each ordered list in `input` (parsed with fancy lists on) reduced to its
    /// `(start, style, delimiter, item count)`.
    fn ordered_lists(input: &str) -> Vec<(i32, ListNumberStyle, ListNumberDelim, usize)> {
        fn collect(
            blocks: &[Block],
            out: &mut Vec<(i32, ListNumberStyle, ListNumberDelim, usize)>,
        ) {
            for block in blocks {
                if let Block::OrderedList(attrs, items) = block {
                    out.push((
                        attrs.start,
                        attrs.style.clone(),
                        attrs.delim.clone(),
                        items.len(),
                    ));
                    for item in items {
                        collect(item, out);
                    }
                }
            }
        }
        let mut out = Vec::new();
        collect(&blocks_with(input, Extension::FancyLists), &mut out);
        out
    }

    #[test]
    fn lowercase_letters_form_an_alphabetic_list() {
        assert_eq!(
            ordered_lists("a. one\nb. two\nc. three\n"),
            [(1, ListNumberStyle::LowerAlpha, ListNumberDelim::Period, 3)]
        );
    }

    #[test]
    fn an_alphabetic_list_starts_at_its_first_letter() {
        assert_eq!(
            ordered_lists("c. three\nd. four\n"),
            [(3, ListNumberStyle::LowerAlpha, ListNumberDelim::Period, 2)]
        );
    }

    #[test]
    fn a_roman_run_is_a_roman_list() {
        assert_eq!(
            ordered_lists("i. one\nii. two\niii. three\niv. four\n"),
            [(1, ListNumberStyle::LowerRoman, ListNumberDelim::Period, 4)]
        );
    }

    #[test]
    fn a_lone_i_opens_a_roman_list() {
        assert_eq!(
            ordered_lists("i. only\n"),
            [(1, ListNumberStyle::LowerRoman, ListNumberDelim::Period, 1)]
        );
    }

    #[test]
    fn an_alphabetic_list_absorbs_a_following_i() {
        // `h. i. j.` is one alphabetic list: `i` continues it as the ninth letter rather than
        // restarting as a roman one.
        assert_eq!(
            ordered_lists("h. eight\ni. nine\nj. ten\n"),
            [(8, ListNumberStyle::LowerAlpha, ListNumberDelim::Period, 3)]
        );
    }

    #[test]
    fn a_multi_letter_roman_does_not_continue_an_alphabetic_list() {
        assert_eq!(
            ordered_lists("a. one\nii. two\n"),
            [
                (1, ListNumberStyle::LowerAlpha, ListNumberDelim::Period, 1),
                (2, ListNumberStyle::LowerRoman, ListNumberDelim::Period, 1),
            ]
        );
    }

    #[test]
    fn a_lone_i_after_a_list_reads_as_the_ninth_letter() {
        // Following another list, the ambiguous `i` resolves to the alphabetic reading.
        assert_eq!(
            ordered_lists("1. one\ni. two\n"),
            [
                (1, ListNumberStyle::Decimal, ListNumberDelim::Period, 1),
                (9, ListNumberStyle::LowerAlpha, ListNumberDelim::Period, 1),
            ]
        );
    }

    #[test]
    fn parenthesized_and_single_paren_delimiters_are_distinguished() {
        assert_eq!(
            ordered_lists("(a) one\n"),
            [(
                1,
                ListNumberStyle::LowerAlpha,
                ListNumberDelim::TwoParens,
                1
            )]
        );
        assert_eq!(
            ordered_lists("a) one\n"),
            [(1, ListNumberStyle::LowerAlpha, ListNumberDelim::OneParen, 1)]
        );
    }

    #[test]
    fn an_uppercase_letter_and_period_need_two_spaces() {
        // One space reads as an ordinary sentence; two spaces make it a list.
        assert!(matches!(
            blocks_with("B. Franklin\n", Extension::FancyLists).as_slice(),
            [Block::Para(_)]
        ));
        assert_eq!(
            ordered_lists("B.  item\n"),
            [(2, ListNumberStyle::UpperAlpha, ListNumberDelim::Period, 1)]
        );
    }

    #[test]
    fn an_uppercase_letter_with_one_space_is_a_list_under_other_delimiters() {
        // The two-space rule guards only the period; a paren delimiter is unambiguous.
        assert_eq!(
            ordered_lists("B) item\n"),
            [(2, ListNumberStyle::UpperAlpha, ListNumberDelim::OneParen, 1)]
        );
    }

    #[test]
    fn only_a_decimal_one_interrupts_a_paragraph() {
        assert!(matches!(
            blocks_with("text\na. item\n", Extension::FancyLists).as_slice(),
            [Block::Para(_)]
        ));
        assert!(matches!(
            blocks_with("text\n1. item\n", Extension::FancyLists).as_slice(),
            [Block::Para(_), Block::OrderedList(..)]
        ));
        assert!(matches!(
            blocks_with("text\n(1) item\n", Extension::FancyLists).as_slice(),
            [Block::Para(_), Block::OrderedList(..)]
        ));
    }

    #[test]
    fn with_the_extension_off_a_letter_marker_is_paragraph_text() {
        assert!(matches!(blocks("a. one\n").as_slice(), [Block::Para(_)]));
    }

    /// Every example list in `input` (parsed with example lists on) as (start, style, delim, item
    /// count), in document order, descendants included.
    fn example_lists(input: &str) -> Vec<(i32, ListNumberStyle, ListNumberDelim, usize)> {
        fn collect(
            blocks: &[Block],
            out: &mut Vec<(i32, ListNumberStyle, ListNumberDelim, usize)>,
        ) {
            for block in blocks {
                match block {
                    Block::OrderedList(attrs, items) => {
                        out.push((
                            attrs.start,
                            attrs.style.clone(),
                            attrs.delim.clone(),
                            items.len(),
                        ));
                        for item in items {
                            collect(item, out);
                        }
                    }
                    Block::BulletList(items) => {
                        for item in items {
                            collect(item, out);
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        collect(&blocks_with(input, Extension::ExampleLists), &mut out);
        out
    }

    /// The flattened text of every top-level paragraph in `input` (example lists on), joined by a
    /// space — enough to observe how `@label` references resolve.
    fn example_text(input: &str) -> String {
        blocks_with(input, Extension::ExampleLists)
            .iter()
            .filter_map(|block| match block {
                Block::Para(inlines) => Some(flatten_inlines(inlines)),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn the_three_example_markers_open_example_lists() {
        use ListNumberDelim::{OneParen, Period, TwoParens};
        use ListNumberStyle::Example;
        assert_eq!(
            example_lists("(@) one\n\n@. two\n\n@) three\n"),
            [
                (1, Example, TwoParens, 1),
                (2, Example, Period, 1),
                (3, Example, OneParen, 1),
            ]
        );
    }

    #[test]
    fn a_reference_resolves_to_its_example_number() {
        assert_eq!(example_text("(@a) apple\n\nSee (@a).\n"), "See (1).");
    }

    #[test]
    fn a_bare_reference_drops_the_parentheses() {
        assert_eq!(example_text("(@a) apple\n\nbare @a end\n"), "bare 1 end");
    }

    #[test]
    fn the_counter_skips_ordinary_ordered_lists() {
        // A plain decimal list between two examples does not advance the example counter.
        assert_eq!(
            example_lists("(@a) x\n\n1. p\n2. q\n\n(@b) y\n"),
            [
                (1, ListNumberStyle::Example, ListNumberDelim::TwoParens, 1),
                (1, ListNumberStyle::Decimal, ListNumberDelim::Period, 2),
                (2, ListNumberStyle::Example, ListNumberDelim::TwoParens, 1),
            ]
        );
        assert_eq!(
            example_text("(@a) x\n\n1. p\n2. q\n\n(@b) y\n\nRefs (@a) and (@b)\n"),
            "Refs (1) and (2)"
        );
    }

    #[test]
    fn a_repeated_label_reuses_its_number() {
        use ListNumberDelim::{OneParen, Period, TwoParens};
        use ListNumberStyle::Example;
        // The second `@a` neither takes a fresh number nor advances the counter, so the distinct
        // label `@b` is two, not three. Three delimiters keep the examples in separate lists.
        assert_eq!(
            example_lists("(@a) x\n\n@a. y\n\n@b) z\n"),
            [
                (1, Example, TwoParens, 1),
                (1, Example, Period, 1),
                (2, Example, OneParen, 1),
            ]
        );
        assert_eq!(
            example_text("(@a) x\n\n@a. y\n\n@b) z\n\nRef (@a) (@b)\n"),
            "Ref (1) (2)"
        );
    }

    #[test]
    fn an_anonymous_example_advances_the_counter() {
        // The unreferenceable `(@)` takes number one, so the following labelled example is two.
        assert_eq!(
            example_lists("(@) x\n\n@a. y\n"),
            [
                (1, ListNumberStyle::Example, ListNumberDelim::TwoParens, 1),
                (2, ListNumberStyle::Example, ListNumberDelim::Period, 1),
            ]
        );
        assert_eq!(example_text("(@) x\n\n@a. y\n\nSee (@a)\n"), "See (2)");
    }

    #[test]
    fn an_anonymous_reference_stays_literal() {
        assert_eq!(example_text("(@) x\n\nSee (@).\n"), "See (@).");
    }

    #[test]
    fn an_undefined_reference_stays_literal() {
        assert_eq!(example_text("(@a) x\n\nSee (@b).\n"), "See (@b).");
    }

    #[test]
    fn a_reference_resolves_within_emphasis_but_not_within_code() {
        // Emphasis content is parsed, so the reference resolves; a code span is verbatim.
        assert_eq!(example_text("(@a) x\n\n*em (@a)*\n"), "em (1)");
        assert_eq!(example_text("(@a) x\n\n`(@a)`\n"), "(@a)");
    }

    #[test]
    fn the_counter_spans_nested_example_lists() {
        // Reading order crosses container boundaries: the example nested in a bullet is two.
        assert_eq!(
            example_text("(@a) x\n\n- bullet\n\n    (@b) nested\n\nRefs (@a) and (@b)\n"),
            "Refs (1) and (2)"
        );
    }

    #[test]
    fn with_the_extension_off_an_example_marker_is_paragraph_text() {
        assert!(matches!(blocks("(@) one\n").as_slice(), [Block::Para(_)]));
        assert!(matches!(blocks("@a. one\n").as_slice(), [Block::Para(_)]));
    }

    fn document(input: &str, exts: &[Extension]) -> carta_ast::Document {
        let mut options = ReaderOptions::default();
        options.extensions = Extensions::from_list(exts);
        CommonmarkReader
            .read(input, &options)
            .expect("reader should not fail")
    }

    /// Parse with greedy paragraphs enabled (the markdown dialect) and the given extensions.
    fn greedy_blocks(input: &str, exts: &[Extension]) -> Vec<Block> {
        let mut options = ReaderOptions::default();
        options.extensions = Extensions::from_list(exts);
        options.greedy_paragraphs = true;
        CommonmarkReader
            .read(input, &options)
            .expect("reader should not fail")
            .blocks
    }

    #[test]
    fn a_greedy_paragraph_folds_a_following_block_quote_heading_and_break() {
        // A block-quote, heading, or thematic-break line right under a paragraph continues it. The
        // block-quote and heading folds are gated on the `blank_before_*` toggles the markdown
        // dialect carries; the thematic break folds on the plain greedy flag.
        let toggles = &[
            Extension::BlankBeforeBlockquote,
            Extension::BlankBeforeHeader,
        ];
        for line in ["> quote", "# heading", "***"] {
            let input = format!("text\n{line}\n");
            assert!(
                matches!(greedy_blocks(&input, toggles).as_slice(), [Block::Para(_)]),
                "expected one paragraph for {input:?}"
            );
        }
    }

    #[test]
    fn a_heading_or_block_quote_interrupts_without_its_blank_before_toggle() {
        // Without `blank_before_header` / `blank_before_blockquote`, the opener interrupts an open
        // paragraph as in strict CommonMark, even where paragraphs are otherwise greedy.
        assert!(matches!(
            greedy_blocks("text\n# heading\n", &[]).as_slice(),
            [Block::Para(_), Block::Header(_, _, _)]
        ));
        assert!(matches!(
            greedy_blocks("text\n> quote\n", &[]).as_slice(),
            [Block::Para(_), Block::BlockQuote(_)]
        ));
        // The thematic break is not toggle-gated, so it still folds into the greedy paragraph.
        assert!(matches!(
            greedy_blocks("text\n***\n", &[]).as_slice(),
            [Block::Para(_)]
        ));
    }

    #[test]
    fn a_greedy_paragraph_is_not_interrupted_by_a_list_marker() {
        // At the top level a fresh list cannot interrupt a paragraph; the marker reads as text.
        assert!(matches!(
            greedy_blocks("text\n- item\n", &[]).as_slice(),
            [Block::Para(_)]
        ));
    }

    #[test]
    fn a_greedy_paragraph_folds_a_fenced_div_and_footnote_definition() {
        assert!(matches!(
            greedy_blocks("text\n::: note\nx\n:::\n", &[Extension::FencedDivs]).as_slice(),
            [Block::Para(_)]
        ));
        assert!(matches!(
            greedy_blocks("text\n[^1]: a note\n", &[Extension::Footnotes]).as_slice(),
            [Block::Para(_)]
        ));
    }

    #[test]
    fn a_fenced_code_block_still_ends_a_greedy_paragraph() {
        assert!(matches!(
            greedy_blocks("text\n```\ncode\n```\n", &[]).as_slice(),
            [Block::Para(_), Block::CodeBlock(_, _)]
        ));
    }

    #[test]
    fn a_blank_line_lets_a_block_open_after_a_greedy_paragraph() {
        assert!(matches!(
            greedy_blocks("text\n\n# heading\n", &[]).as_slice(),
            [Block::Para(_), Block::Header(_, _, _)]
        ));
        assert!(matches!(
            greedy_blocks("text\n\n- item\n", &[]).as_slice(),
            [Block::Para(_), Block::BulletList(_)]
        ));
    }

    #[test]
    fn sibling_list_items_are_not_folded_into_each_other() {
        // Greediness suppresses only a fresh list interrupting a paragraph, never the markers that
        // continue an open list.
        let blocks = greedy_blocks("- a\n- b\n", &[]);
        let [Block::BulletList(items)] = blocks.as_slice() else {
            panic!("expected a bullet list");
        };
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn a_sublist_opens_under_an_item_regardless_of_its_start_number() {
        // An indented ordered marker opens a sublist even when it does not start at one.
        let blocks = greedy_blocks("1. a\n   3. b\n", &[Extension::FancyLists]);
        let [Block::OrderedList(_, items)] = blocks.as_slice() else {
            panic!("expected an ordered list");
        };
        let [first] = items.as_slice() else {
            panic!("expected one outer item");
        };
        assert!(
            first
                .iter()
                .any(|block| matches!(block, Block::OrderedList(_, _))),
            "the item should contain a nested ordered list"
        );
    }

    #[test]
    fn a_yaml_metadata_block_populates_meta_and_is_removed_from_the_body() {
        use carta_ast::MetaValue;
        let doc = document(
            "---\ntitle: A Note\nflag: true\nempty: ~\nrevision: 007\n---\n\nBody.\n",
            &[Extension::YamlMetadataBlock],
        );
        assert!(matches!(
            doc.meta.get("title"),
            Some(MetaValue::MetaInlines(_))
        ));
        assert_eq!(doc.meta.get("flag"), Some(&MetaValue::MetaBool(true)));
        assert_eq!(
            doc.meta.get("empty"),
            Some(&MetaValue::MetaString(String::new()))
        );
        // An unquoted numeric scalar is canonicalized before it is parsed as inline markdown.
        assert_eq!(
            doc.meta.get("revision"),
            Some(&MetaValue::MetaInlines(vec![Inline::Str("7".to_owned())]))
        );
        assert!(matches!(doc.blocks.as_slice(), [Block::Para(_)]));
    }

    #[test]
    fn a_yaml_block_without_a_closing_fence_is_not_metadata() {
        let doc = document(
            "---\ntitle: A Note\n\nBody.\n",
            &[Extension::YamlMetadataBlock],
        );
        assert!(doc.meta.is_empty());
    }

    #[test]
    fn yaml_metadata_is_inert_without_the_extension() {
        let doc = document("---\nk: v\n---\n\nBody.\n", &[]);
        assert!(doc.meta.is_empty());
    }

    #[test]
    fn a_title_block_sets_title_author_and_date() {
        use carta_ast::MetaValue;
        let doc = document(
            "% A Note\n% Ada; Grace\n% 2026\n\nBody.\n",
            &[Extension::PandocTitleBlock],
        );
        assert!(matches!(
            doc.meta.get("title"),
            Some(MetaValue::MetaInlines(_))
        ));
        match doc.meta.get("author") {
            Some(MetaValue::MetaList(authors)) => assert_eq!(authors.len(), 2),
            other => panic!("expected two authors, got {other:?}"),
        }
        assert!(matches!(
            doc.meta.get("date"),
            Some(MetaValue::MetaInlines(_))
        ));
        assert!(matches!(doc.blocks.as_slice(), [Block::Para(_)]));
    }

    #[test]
    fn malformed_yaml_metadata_is_an_error() {
        let mut options = ReaderOptions::default();
        options.extensions = Extensions::from_list(&[Extension::YamlMetadataBlock]);
        let error = CommonmarkReader
            .read("---\nx: [\n---\n\nBody.\n", &options)
            .expect_err("malformed metadata should fail");
        assert!(matches!(error, carta_core::Error::InvalidMetadata(_)));
    }

    /// The inline caption of the first block, or `None` when that block is not a table or carries no
    /// caption.
    fn caption_inlines(blocks: &[Block]) -> Option<&[Inline]> {
        let Block::Table(table) = blocks.first()? else {
            return None;
        };
        match table.caption.long.as_slice() {
            [Block::Plain(inlines)] => Some(inlines),
            _ => None,
        }
    }

    #[test]
    fn a_pipe_table_takes_a_below_caption() {
        let doc = document(
            "| a | b |\n|---|---|\n| 1 | 2 |\n\nTable: A caption.\n",
            &[Extension::PipeTables, Extension::TableCaptions],
        );
        assert!(matches!(doc.blocks.as_slice(), [Block::Table(_)]));
        let inlines = caption_inlines(&doc.blocks).expect("captioned table");
        assert_eq!(inlines.first(), Some(&Inline::Str("A".to_owned())));
    }

    #[test]
    fn a_simple_table_takes_an_above_caption() {
        let doc = document(
            "table: Above it.\n\nName   Age\n----   ---\nAnn    9\n",
            &[Extension::SimpleTables, Extension::TableCaptions],
        );
        assert!(matches!(doc.blocks.as_slice(), [Block::Table(_)]));
        assert!(caption_inlines(&doc.blocks).is_some());
    }

    #[test]
    fn a_multiline_caption_folds_across_lines() {
        let doc = document(
            "| a | b |\n|---|---|\n| 1 | 2 |\n\nTable: First line\nsecond line.\n",
            &[Extension::PipeTables, Extension::TableCaptions],
        );
        let inlines = caption_inlines(&doc.blocks).expect("captioned table");
        assert!(inlines.contains(&Inline::SoftBreak));
    }

    #[test]
    fn a_bare_colon_below_a_pipe_table_is_a_caption_not_a_definition() {
        // The `:` marker also opens a definition list; below a pipe table it is the table's caption,
        // so the table must survive rather than collapsing into a definition term.
        let doc = document(
            "| a | b |\n|---|---|\n| 1 | 2 |\n\n: A bare-colon caption.\n",
            &[
                Extension::PipeTables,
                Extension::TableCaptions,
                Extension::DefinitionLists,
            ],
        );
        assert!(matches!(doc.blocks.as_slice(), [Block::Table(_)]));
        assert!(caption_inlines(&doc.blocks).is_some());
    }

    #[test]
    fn an_uppercase_table_marker_is_not_a_caption() {
        let doc = document(
            "| a | b |\n|---|---|\n| 1 | 2 |\n\nTABLE: not a caption\n",
            &[Extension::PipeTables, Extension::TableCaptions],
        );
        assert!(matches!(
            doc.blocks.as_slice(),
            [Block::Table(_), Block::Para(_)]
        ));
        assert!(caption_inlines(&doc.blocks).is_none());
    }

    #[test]
    fn an_ordinary_definition_list_is_unaffected_by_caption_handling() {
        let doc = document(
            "Term\n\n: Its definition.\n",
            &[
                Extension::PipeTables,
                Extension::TableCaptions,
                Extension::DefinitionLists,
            ],
        );
        assert!(matches!(doc.blocks.as_slice(), [Block::DefinitionList(_)]));
    }

    /// The inlines of a single-paragraph markdown-dialect document, for inline assertions.
    fn md_para(input: &str, exts: &[Extension]) -> Vec<Inline> {
        match read_markdown(input, exts).blocks.as_slice() {
            [Block::Para(inlines)] => inlines.clone(),
            other => panic!("expected a single paragraph, got {other:?}"),
        }
    }

    // --- Gap 1: triple-emphasis nests strong on the outside, emphasis on the inside ---

    #[test]
    fn markdown_nests_strong_outside_emph_for_a_triple_run() {
        let inlines = md_para("***both***\n", &[]);
        assert!(
            matches!(
                inlines.as_slice(),
                [Inline::Strong(inner)]
                    if matches!(inner.as_slice(), [Inline::Emph(text)]
                        if matches!(text.as_slice(), [Inline::Str(s)] if s == "both"))
            ),
            "expected Strong[Emph[both]], got {inlines:?}"
        );
    }

    #[test]
    fn markdown_keeps_a_run_of_four_delimiters_literal() {
        // Four `*` open no emphasis in the markdown dialect; the run stays text.
        let inlines = md_para("****a****\n", &[]);
        assert!(
            matches!(inlines.as_slice(), [Inline::Str(s)] if s == "****a****"),
            "expected literal text, got {inlines:?}"
        );
    }

    #[test]
    fn markdown_underscore_triple_run_also_nests_strong_outside() {
        let inlines = md_para("___both___\n", &[]);
        assert!(
            matches!(
                inlines.as_slice(),
                [Inline::Strong(inner)] if matches!(inner.as_slice(), [Inline::Emph(_)])
            ),
            "expected Strong[Emph[..]], got {inlines:?}"
        );
    }

    // --- Gap 2: explicit angle autolinks carry a uri/email class ---

    fn single_link(inlines: &[Inline]) -> Option<(&Attr, &Target)> {
        match inlines {
            [Inline::Link(attr, _, target)] => Some((attr, target)),
            _ => None,
        }
    }

    #[test]
    fn markdown_uri_autolink_carries_the_uri_class() {
        let inlines = md_para("<http://example.com>\n", &[]);
        let (attr, target) = single_link(&inlines).expect("a single link");
        assert_eq!(attr.classes, vec!["uri".to_owned()]);
        assert_eq!(target.url, "http://example.com");
    }

    #[test]
    fn markdown_email_autolink_carries_the_email_class_and_mailto_url() {
        let inlines = md_para("<a@b.com>\n", &[]);
        let (attr, target) = single_link(&inlines).expect("a single link");
        assert_eq!(attr.classes, vec!["email".to_owned()]);
        assert_eq!(target.url, "mailto:a@b.com");
    }

    #[test]
    fn markdown_scheme_autolink_carries_the_uri_class() {
        for input in ["<ftp://x.y>\n", "<mailto:a@b.com>\n", "<tel:+123>\n"] {
            let inlines = md_para(input, &[]);
            let (attr, _) = single_link(&inlines).expect("a single link");
            assert_eq!(attr.classes, vec!["uri".to_owned()], "for {input:?}");
        }
    }

    #[test]
    fn commonmark_angle_autolink_carries_no_class() {
        // In the strict CommonMark dialect the autolink class list is empty.
        let inlines = match blocks("<http://example.com>\n").as_slice() {
            [Block::Para(inlines)] => inlines.clone(),
            other => panic!("expected a paragraph, got {other:?}"),
        };
        let (attr, _) = single_link(&inlines).expect("a single link");
        assert!(
            attr.classes.is_empty(),
            "expected empty classes, got {attr:?}"
        );
    }

    // --- Gap 5: balanced parentheses inside an inline link destination ---

    #[test]
    fn markdown_link_destination_keeps_balanced_inner_parentheses() {
        let inlines = md_para("[c](/u (d))\n", &[]);
        let (_, target) = single_link(&inlines).expect("a single link");
        // The space is percent-encoded and the inner `(d)` is part of the destination.
        assert_eq!(target.url, "/u%20(d)");
        assert_eq!(target.title, "");
    }

    #[test]
    fn markdown_link_destination_separates_a_trailing_title() {
        let inlines = md_para("[c](/u (d) \"t\")\n", &[]);
        let (_, target) = single_link(&inlines).expect("a single link");
        assert_eq!(target.url, "/u%20(d)");
        assert_eq!(target.title, "t");
    }

    #[test]
    fn markdown_link_destination_keeps_nested_balanced_parentheses() {
        let inlines = md_para("[c](/u(a(b)c)d)\n", &[]);
        let (_, target) = single_link(&inlines).expect("a single link");
        assert_eq!(target.url, "/u(a(b)c)d");
    }

    // --- Gap 6: tilde delimiter runs resolve to subscript or strikeout ---

    #[test]
    fn markdown_single_tilde_pair_is_a_subscript() {
        let inlines = md_para("z ~x~\n", &[Extension::Subscript, Extension::Strikeout]);
        assert!(
            inlines.iter().any(|i| matches!(i, Inline::Subscript(_))),
            "expected a subscript, got {inlines:?}"
        );
    }

    #[test]
    fn markdown_double_tilde_pair_is_a_strikeout() {
        let inlines = md_para("z ~~x~~\n", &[Extension::Subscript, Extension::Strikeout]);
        assert!(
            inlines.iter().any(|i| matches!(i, Inline::Strikeout(_))),
            "expected a strikeout, got {inlines:?}"
        );
    }

    #[test]
    fn markdown_triple_tilde_run_collapses_to_a_single_subscript() {
        // The whole odd run is consumed into one subscript; no strikeout nests inside it.
        let inlines = md_para(
            "z ~~~triple~~~\n",
            &[Extension::Subscript, Extension::Strikeout],
        );
        let sub = inlines
            .iter()
            .find_map(|i| match i {
                Inline::Subscript(content) => Some(content.clone()),
                _ => None,
            })
            .expect("a subscript");
        assert!(
            matches!(sub.as_slice(), [Inline::Str(s)] if s == "triple"),
            "expected Subscript[triple], got {sub:?}"
        );
        assert!(
            !inlines.iter().any(|i| matches!(i, Inline::Strikeout(_))),
            "a triple-tilde run should not form a strikeout: {inlines:?}"
        );
    }
}
