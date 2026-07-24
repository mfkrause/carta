//! Shared helpers for the HTML reader tests.

use super::super::HtmlReader;
use carta_ast::{Block, Inline};
use carta_core::{Extension, Extensions, Reader, ReaderOptions};

/// The structural extensions enabled by default for the `html` format. The unit tests exercise
/// this default dialect; `+`/`-` toggle behavior is covered by the golden corpus.
pub(super) fn html_defaults() -> Extensions {
    Extensions::from_list(&[
        Extension::AutoIdentifiers,
        Extension::LineBlocks,
        Extension::NativeDivs,
        Extension::NativeSpans,
    ])
}

pub(super) fn read_with(input: &str, extensions: Extensions) -> Vec<Block> {
    let mut options = ReaderOptions::default();
    options.extensions = extensions;
    HtmlReader
        .read(input, &options)
        .expect("reader should not fail")
        .blocks
}

pub(super) fn blocks(input: &str) -> Vec<Block> {
    read_with(input, html_defaults())
}

pub(super) fn first_block(input: &str) -> Block {
    blocks(input).into_iter().next().expect("a block")
}

pub(super) fn para_inlines(input: &str) -> Vec<Inline> {
    match first_block(input) {
        Block::Para(inlines) | Block::Plain(inlines) => inlines,
        other => panic!("expected a paragraph, got {other:?}"),
    }
}

/// Read with the `html` default set plus the given text extensions, which is what `html+smart`
/// and the `html+tex_math_*` corpus specs resolve to.
pub(super) fn read_with_text_ext(input: &str, added: &[Extension]) -> Vec<Block> {
    read_with(input, html_defaults().union(Extensions::from_list(added)))
}

pub(super) fn para_inlines_ext(input: &str, added: &[Extension]) -> Vec<Inline> {
    match read_with_text_ext(input, added).into_iter().next() {
        Some(Block::Para(inlines) | Block::Plain(inlines)) => inlines,
        other => panic!("expected a paragraph, got {other:?}"),
    }
}
