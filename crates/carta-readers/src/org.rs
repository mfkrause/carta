//! Org reader: parses Org markup into the document model.
//!
//! Parsing is two-phase. A line-oriented block pass consumes the input into [`Block`]s, dispatching
//! on each line's opening: headlines (`* `), greater blocks (`#+begin_…`/`#+end_…`), keyword lines
//! (`#+key: value`), tables (`|`), lists, drawers, fixed-width (`: `) and comment (`# `) lines, and
//! everything else as a paragraph. A second, per-fragment pass then scans each paragraph, headline,
//! cell, and item into [`Inline`](carta_ast::Inline)s: emphasis, verbatim, sub/superscripts, links,
//! footnotes, math,
//! entities, and citations.
//!
//! Footnote definitions are gathered up front and their references resolved inline, so a `[fn:label]`
//! reference expands to a [`Inline::Note`](carta_ast::Inline::Note) carrying the definition's blocks.

use std::borrow::Cow;
use std::collections::BTreeMap;

use carta_ast::{Block, Document, MetaValue, Text, slug, slug_gfm};
use carta_core::{Extension, Extensions, Reader, ReaderOptions, Result};

use crate::heading_ids::{IdRegistry, IdScheme};
use crate::transliterate::fold_to_ascii;

use blocks::{headline_level, parse_blocks};

mod blocks;
mod citations;
mod drawers;
mod entities;
mod greater;
mod inline;
mod inline_helpers;
mod keyword;
mod lists;
mod tables;

#[cfg(test)]
mod tests;

/// Parses Org markup into the document model.
///
/// The default extension set enables auto identifiers, citations, task-list checkboxes, and the
/// typographic replacements of `special_strings`; `smart` adds curly quotes, `fancy_lists` numbered
/// list markers, and `gfm_auto_identifiers`/`ascii_identifiers` alternate identifier shapes.
#[derive(Debug, Default, Clone, Copy)]
pub struct OrgReader;

impl Reader for OrgReader {
    fn read(&self, input: &str, options: &ReaderOptions) -> Result<Document> {
        let ext = options.extensions;
        let normalized = normalize(input);
        let lines: Vec<&str> = normalized.split('\n').collect();

        let (body_lines, defs) = collect_footnotes(&lines);

        // Bodies parse first so references carry definition blocks; nested references see an empty table.
        let empty_notes: BTreeMap<String, Vec<Block>> = BTreeMap::new();
        let mut notes: BTreeMap<String, Vec<Block>> = BTreeMap::new();
        for (label, text) in &defs {
            let def_lines: Vec<&str> = text.split('\n').collect();
            let mut throwaway_ids = new_id_registry();
            let mut throwaway_meta = BTreeMap::new();
            let blocks = parse_blocks(
                &def_lines,
                ext,
                &empty_notes,
                &mut throwaway_ids,
                &mut throwaway_meta,
            );
            notes.insert(label.clone(), blocks);
        }

        let mut ids = new_id_registry();
        let mut meta: BTreeMap<Text, MetaValue> = BTreeMap::new();
        let blocks = parse_blocks(&body_lines, ext, &notes, &mut ids, &mut meta);

        Ok(Document {
            meta,
            blocks,
            ..Document::default()
        })
    }
}

/// Normalizes line endings to `\n` so the line-oriented pass sees a single terminator. Input without
/// a carriage return is already normalized and is borrowed unchanged.
fn normalize(input: &str) -> Cow<'_, str> {
    if input.contains('\r') {
        Cow::Owned(input.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        Cow::Borrowed(input)
    }
}

// -- Footnote gathering ------------------------------------------------------------------------

/// Splits block-level footnote definitions (`[fn:label] …`) out of the line stream, returning the
/// remaining body lines and the ordered `(label, joined-body)` definitions. A definition's body
/// continues across single blank lines, so it can hold several blocks; it ends at the next footnote
/// definition, a headline, two consecutive blank lines, or the end of input.
fn collect_footnotes<'a>(lines: &[&'a str]) -> (Vec<&'a str>, Vec<(String, String)>) {
    let mut body = Vec::new();
    let mut defs = Vec::new();
    let mut i = 0;
    while let Some(line) = lines.get(i) {
        if let Some((label, first)) = footnote_definition(line) {
            let mut collected = vec![first];
            i += 1;
            while let Some(next) = lines.get(i) {
                if footnote_definition(next).is_some() || headline_level(next).is_some() {
                    break;
                }
                if next.trim().is_empty()
                    && lines
                        .get(i + 1)
                        .is_none_or(|following| following.trim().is_empty())
                {
                    break;
                }
                collected.push((*next).to_owned());
                i += 1;
            }
            defs.push((label, collected.join("\n")));
        } else {
            body.push(*line);
            i += 1;
        }
    }
    (body, defs)
}

/// Recognizes a block-level footnote definition `[fn:label] rest`, returning the label and the text
/// after the closing bracket.
fn footnote_definition(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("[fn:")?;
    let close = rest.find(']')?;
    let label = &rest[..close];
    if label.is_empty() || !label.chars().all(is_footnote_label_char) {
        return None;
    }
    let after = rest.get(close + 1..).unwrap_or("");
    Some((label.to_owned(), after.trim_start().to_owned()))
}

fn is_footnote_label_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-')
}

// -- Identifier derivation ---------------------------------------------------------------------

/// A fresh heading-identifier registry with `section` reserved from the start, so the first heading
/// that reduces to it is already `section-1`.
fn new_id_registry() -> IdRegistry {
    let mut ids = IdRegistry::default();
    ids.reserve_native("section");
    ids
}

/// Derives an identifier for `text` under the active extensions, or an empty string when no
/// auto-identifier extension is on. The slug shape follows the extension, but headings always
/// disambiguate natively: an empty slug becomes `section` and repeats increment until unused.
fn assign_id(ids: &mut IdRegistry, text: &str, ext: Extensions) -> String {
    let Some(scheme) = IdScheme::select(ext, true) else {
        return String::new();
    };
    let folded;
    let source = if ext.contains(Extension::AsciiIdentifiers) {
        folded = fold_to_ascii(text);
        folded.as_str()
    } else {
        text
    };
    let base = match scheme {
        IdScheme::Plain => slug(source),
        IdScheme::Gfm => slug_gfm(source),
    };
    ids.assign_native(base)
}

// -- Prefix helpers ----------------------------------------------------------------------------

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len() && s.get(..prefix.len())?.eq_ignore_ascii_case(prefix) {
        s.get(prefix.len()..)
    } else {
        None
    }
}

fn starts_with_ci(s: &str, prefix: &str) -> bool {
    strip_prefix_ci(s, prefix).is_some()
}
