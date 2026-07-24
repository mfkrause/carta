//! Shared helpers and topic modules for the `CommonMark` reader tests.

use super::CommonmarkReader;
use carta_ast::{Attr, Block, Document, Inline, ListNumberDelim, ListNumberStyle, Target};
use carta_core::{Extension, Extensions, Reader, ReaderOptions};

mod fancy_example_lists;
mod footnotes_lists_divs;
mod greedy_paragraphs;
mod identifiers_references;
mod line_blocks_definitions;
mod metadata_tables_markdown;

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

fn header_ids(blocks: &[Block]) -> Vec<String> {
    blocks
        .iter()
        .filter_map(|b| match b {
            Block::Header(_, attr, _) => Some(attr.id.to_string()),
            _ => None,
        })
        .collect()
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
                    out.push(target.url.to_string());
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

fn cite_note_nums(blocks: &[Block]) -> Vec<i32> {
    fn collect(inlines: &[Inline], out: &mut Vec<i32>) {
        for inline in inlines {
            if let Inline::Cite(citations, _) = inline {
                out.extend(citations.iter().map(|c| c.note_num));
            }
        }
    }
    let mut out = Vec::new();
    for block in blocks {
        match block {
            Block::Header(_, _, inlines) | Block::Para(inlines) => collect(inlines, &mut out),
            _ => {}
        }
    }
    out
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
            Inline::Emph(children) | Inline::Strong(children) | Inline::Link(_, children, _) => {
                out.push_str(&flatten_inlines(children));
            }
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

/// Each ordered list in `input` (parsed with fancy lists on) reduced to its
/// `(start, style, delimiter, item count)`.
fn ordered_lists(input: &str) -> Vec<(i32, ListNumberStyle, ListNumberDelim, usize)> {
    fn collect(blocks: &[Block], out: &mut Vec<(i32, ListNumberStyle, ListNumberDelim, usize)>) {
        for block in blocks {
            if let Block::OrderedList(attrs, items) = block {
                out.push((attrs.start, attrs.style, attrs.delim, items.len()));
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

/// Every example list in `input` (parsed with example lists on) as (start, style, delim, item
/// count), in document order, descendants included.
fn example_lists(input: &str) -> Vec<(i32, ListNumberStyle, ListNumberDelim, usize)> {
    fn collect(blocks: &[Block], out: &mut Vec<(i32, ListNumberStyle, ListNumberDelim, usize)>) {
        for block in blocks {
            match block {
                Block::OrderedList(attrs, items) => {
                    out.push((attrs.start, attrs.style, attrs.delim, items.len()));
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
/// space, enough to observe how `@label` references resolve.
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

/// The inlines of a single-paragraph markdown-dialect document, for inline assertions.
fn md_para(input: &str, exts: &[Extension]) -> Vec<Inline> {
    match read_markdown(input, exts).blocks.as_slice() {
        [Block::Para(inlines)] => inlines.clone(),
        other => panic!("expected a single paragraph, got {other:?}"),
    }
}

fn single_link(inlines: &[Inline]) -> Option<(&Attr, &Target)> {
    match inlines {
        [Inline::Link(attr, _, target)] => Some((attr, target)),
        _ => None,
    }
}
