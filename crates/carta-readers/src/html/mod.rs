//! HTML reader.
//!
//! Document metadata is read from a `<head>` element when present.

mod classify;
mod convert;
mod mathml;
mod notes;
mod table;
mod tokenize;
mod tree;

use std::borrow::Cow;

use carta_ast::Document;
use carta_core::{Extensions, Reader, ReaderOptions, Result};

#[cfg(feature = "opml")]
use carta_ast::Inline;

#[cfg(feature = "epub")]
pub(crate) use convert::escape_uri;
#[cfg(feature = "opml")]
use convert::inlines_from_nodes;
use convert::{Converter, Flow, extract_meta};
use tokenize::tokenize;
use tree::{build_tree, locate};

/// Parses HTML text into the document model.
#[derive(Debug, Default, Clone, Copy)]
pub struct HtmlReader;

impl Reader for HtmlReader {
    fn read(&self, input: &str, options: &ReaderOptions) -> Result<Document> {
        Ok(parse(input, options.extensions))
    }
}

fn parse(input: &str, ext: Extensions) -> Document {
    let normalized = normalize(input);
    let tokens = tokenize(&normalized);
    let roots = build_tree(tokens);
    let (head, body) = locate(&roots);

    let mut converter = Converter::new(ext);
    converter.index_notes(notes::collect_note_defs(&body));
    let meta = head.map(extract_meta).unwrap_or_default();
    let blocks = converter.blocks(&body, Flow::Prose);
    Document {
        meta: meta.into_iter().map(|(k, v)| (k.into(), v)).collect(),
        blocks,
        ..Document::default()
    }
}

/// Parse a string of HTML inline markup into inlines, with no surrounding block. Recognized inline
/// tags (`<em>`, `<strong>`, `<code>`, `<a>`, …) become their corresponding constructs, character
/// references are resolved, and leading and trailing whitespace is trimmed. Intended for callers
/// that carry inline content in a single string, such as an outline heading.
#[cfg(feature = "opml")]
pub(crate) fn parse_inline_fragment(input: &str) -> Vec<Inline> {
    let normalized = normalize(input);
    let tokens = tokenize(&normalized);
    let roots = build_tree(tokens);
    inlines_from_nodes(&roots)
}

/// Normalize line endings to `\n` and strip a leading byte-order mark.
fn normalize(input: &str) -> Cow<'_, str> {
    let without_bom = input.strip_prefix('\u{feff}').unwrap_or(input);
    if !without_bom.contains('\r') {
        return Cow::Borrowed(without_bom);
    }
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
    Cow::Owned(out)
}

#[cfg(test)]
mod reader_tests;
