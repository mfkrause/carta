//! Lists: bullet, ordered, definition, task-list checkboxes, and tight/loose handling.

use std::collections::BTreeMap;

use carta_ast::{Block, Inline, ListAttributes, ListNumberDelim, ListNumberStyle, MetaValue, Text};
use carta_core::{Extension, Extensions};

use crate::heading_ids::IdRegistry;

use super::blocks::parse_blocks;
use super::inline::parse_inlines;

/// The kind of a list item marker.
#[derive(Clone, Copy, PartialEq)]
enum Marker {
    Bullet,
    Ordered(ListNumberStyle, ListNumberDelim),
}

/// A recognized list marker: its column, the width consumed by the marker plus following space, and
/// the marker kind.
pub(super) struct MarkerInfo {
    indent: usize,
    content_col: usize,
    kind: Marker,
}

pub(super) fn list_marker(line: &str) -> Option<MarkerInfo> {
    let indent = line.len() - line.trim_start().len();
    let rest = line.get(indent..)?;
    let bytes = rest.as_bytes();
    // Bullet: '-' or '+', or '*' only when indented.
    if let Some(&c) = bytes.first()
        && (matches!(c, b'-' | b'+') || (c == b'*' && indent > 0))
        && (bytes.get(1) == Some(&b' ') || bytes.len() == 1)
    {
        return Some(MarkerInfo {
            indent,
            content_col: indent + 2,
            kind: Marker::Bullet,
        });
    }
    // Ordered: digits or a single letter, then '.' or ')'.
    let mut j = 0;
    while bytes.get(j).is_some_and(u8::is_ascii_digit) {
        j += 1;
    }
    let style = if j > 0 {
        ListNumberStyle::Decimal
    } else if let Some(&letter) = bytes
        .first()
        .filter(|c| c.is_ascii_alphabetic())
        .filter(|_| bytes.get(1).is_some_and(|&c| c == b'.' || c == b')'))
    {
        j = 1;
        if letter.is_ascii_uppercase() {
            ListNumberStyle::UpperAlpha
        } else {
            ListNumberStyle::LowerAlpha
        }
    } else {
        return None;
    };
    let delim = match bytes.get(j) {
        Some(b'.') => ListNumberDelim::Period,
        Some(b')') => ListNumberDelim::OneParen,
        _ => return None,
    };
    if bytes.get(j + 1) == Some(&b' ') || bytes.len() == j + 1 {
        Some(MarkerInfo {
            indent,
            content_col: indent + j + 2,
            kind: Marker::Ordered(style, delim),
        })
    } else {
        None
    }
}

pub(super) fn parse_list(
    lines: &[&str],
    start: usize,
    ext: Extensions,
    notes: &BTreeMap<String, Vec<Block>>,
    ids: &mut IdRegistry,
    meta: &mut BTreeMap<Text, MetaValue>,
) -> (Option<Block>, usize) {
    let Some(first) = list_marker(lines.get(start).copied().unwrap_or("")) else {
        return (None, 1);
    };
    let base_indent = first.indent;
    let first_kind = first.kind;

    let mut items: Vec<Vec<&str>> = Vec::new();
    let mut loose = false;
    let mut i = start;
    let mut pending_blank = false;

    while let Some(&line) = lines.get(i) {
        if line.trim().is_empty() {
            pending_blank = true;
            i += 1;
            continue;
        }
        if let Some(marker) = list_marker(line)
            && marker.indent == base_indent
            && same_series(first_kind, marker.kind)
        {
            if pending_blank && !items.is_empty() {
                loose = true;
            }
            pending_blank = false;
            let content_col = marker.content_col;
            let mut item_lines = vec![line.get(content_col..).unwrap_or("")];
            i += 1;
            while let Some(&next) = lines.get(i) {
                if next.trim().is_empty() {
                    pending_blank = true;
                    item_lines.push("");
                    i += 1;
                    continue;
                }
                let next_indent = next.len() - next.trim_start().len();
                let is_sibling = list_marker(next).is_some_and(|m| m.indent == base_indent);
                if next_indent > base_indent && !is_sibling {
                    if pending_blank {
                        loose = true;
                    }
                    pending_blank = false;
                    item_lines.push(dedent_line(next, content_col));
                    i += 1;
                } else {
                    break;
                }
            }
            while item_lines.last() == Some(&"") {
                item_lines.pop();
            }
            items.push(item_lines);
            continue;
        }
        break;
    }

    if items.is_empty() {
        return (None, 1);
    }

    if let Some(defs) = try_definition_list(&items, ext, notes, ids, meta, loose) {
        return (Some(defs), i - start);
    }

    let item_blocks: Vec<Vec<Block>> = items
        .iter()
        .map(|item| {
            let blocks = parse_list_item(item, ext, notes, ids, meta);
            if loose { blocks } else { tighten(blocks) }
        })
        .collect();

    let block = match first_kind {
        Marker::Bullet => Block::BulletList(item_blocks),
        Marker::Ordered(style, delim) => {
            let (style, delim) = if ext.contains(Extension::FancyLists) {
                (style, delim)
            } else {
                (ListNumberStyle::DefaultStyle, ListNumberDelim::DefaultDelim)
            };
            Block::OrderedList(
                ListAttributes {
                    start: 1,
                    style,
                    delim,
                },
                item_blocks,
            )
        }
    };
    (Some(block), i - start)
}

/// Whether two markers belong to the same list (both bullets, or both ordered).
fn same_series(a: Marker, b: Marker) -> bool {
    matches!(
        (a, b),
        (Marker::Bullet, Marker::Bullet) | (Marker::Ordered(..), Marker::Ordered(..))
    )
}

fn parse_list_item(
    item: &[&str],
    ext: Extensions,
    notes: &BTreeMap<String, Vec<Block>>,
    ids: &mut IdRegistry,
    meta: &mut BTreeMap<Text, MetaValue>,
) -> Vec<Block> {
    let mut lines = item.to_vec();
    let mut checkbox = None;
    if ext.contains(Extension::TaskLists)
        && let Some(first) = lines.first_mut()
        && let Some((glyph, rest)) = strip_checkbox(first)
    {
        checkbox = Some(glyph);
        *first = rest;
    }
    let mut blocks = parse_blocks(&lines, ext, notes, ids, meta);
    if let Some(glyph) = checkbox {
        prepend_checkbox(&mut blocks, glyph);
    }
    blocks
}

/// Splits a leading `[ ]`/`[X]`/`[-]` checkbox off a list item's first line, returning its ballot
/// glyph and the remaining text. The checkbox must be followed by a space or end the line.
fn strip_checkbox(line: &str) -> Option<(&'static str, &str)> {
    for (token, glyph) in [
        ("[ ]", "\u{2610}"),
        ("[-]", "\u{2610}"),
        ("[X]", "\u{2612}"),
    ] {
        if let Some(rest) = line.strip_prefix(token) {
            if rest.is_empty() {
                return Some((glyph, rest));
            }
            if let Some(after) = rest.strip_prefix(' ') {
                return Some((glyph, after));
            }
        }
    }
    None
}

/// Prepends a checkbox glyph to a list item's first inline-bearing block, or introduces a plain block
/// when the item has no content.
fn prepend_checkbox(blocks: &mut Vec<Block>, glyph: &str) {
    match blocks.first_mut() {
        Some(Block::Plain(inlines) | Block::Para(inlines)) => {
            inlines.splice(0..0, [Inline::Str(glyph.into()), Inline::Space]);
        }
        _ => blocks.insert(0, Block::Plain(vec![Inline::Str(glyph.into())])),
    }
}

/// Converts leading paragraphs to plain blocks for a tight list.
fn tighten(blocks: Vec<Block>) -> Vec<Block> {
    blocks
        .into_iter()
        .map(|b| match b {
            Block::Para(inlines) => Block::Plain(inlines),
            other => other,
        })
        .collect()
}

fn try_definition_list(
    items: &[Vec<&str>],
    ext: Extensions,
    notes: &BTreeMap<String, Vec<Block>>,
    ids: &mut IdRegistry,
    meta: &mut BTreeMap<Text, MetaValue>,
    loose: bool,
) -> Option<Block> {
    let first = items.first()?;
    split_definition(first.first().copied().unwrap_or(""))?;
    let mut entries = Vec::new();
    for item in items {
        let head = item.first().copied().unwrap_or("");
        let (term_text, def_first) = match split_definition(head) {
            Some(pair) => pair,
            None => (head, ""),
        };
        let term = parse_inlines(term_text.trim(), ext, notes);
        let mut def_lines = vec![def_first];
        def_lines.extend(item.get(1..).unwrap_or(&[]).iter().copied());
        let blocks = parse_blocks(&def_lines, ext, notes, ids, meta);
        let blocks = if loose { blocks } else { tighten(blocks) };
        entries.push((term, vec![blocks]));
    }
    Some(Block::DefinitionList(entries))
}

/// Splits a definition-list item head `term :: definition` into its term and the start of its
/// definition.
fn split_definition(line: &str) -> Option<(&str, &str)> {
    let idx = line.find(" :: ")?;
    Some((line.get(..idx)?, line.get(idx + 4..)?))
}

/// Removes up to `col` leading spaces from a continuation line, borrowing the remaining slice.
fn dedent_line(line: &str, col: usize) -> &str {
    let indent = line.len() - line.trim_start().len();
    let drop = indent.min(col);
    line.get(drop..).unwrap_or("")
}
