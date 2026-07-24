//! Citation payloads: `[cite:...]` items, styles, and modes.

use std::collections::BTreeMap;

use carta_ast::{Block, Inline};
use carta_core::Extensions;

use super::inline::parse_inlines;

pub(super) fn parse_citation_items(
    payload: &str,
    style: Option<&str>,
    ext: Extensions,
    notes: &BTreeMap<String, Vec<Block>>,
) -> Option<Vec<carta_ast::Citation>> {
    let mode = citation_mode(style);
    let chunks: Vec<&str> = payload.split(';').collect();

    let mut prefix_carry: Option<&str> = None;
    let mut items: Vec<(String, Vec<Inline>, Vec<Inline>)> = Vec::new();
    let mut trailing_suffix: Option<&str> = None;

    for chunk in chunks {
        match chunk.find('@') {
            Some(at) => {
                let prefix = chunk.get(..at).unwrap_or("");
                let after = chunk.get(at + 1..).unwrap_or("");
                let key_end = after
                    .find(|c: char| !is_citation_key_char(c))
                    .unwrap_or(after.len());
                let key = after.get(..key_end).unwrap_or("").to_owned();
                let suffix = after.get(key_end..).unwrap_or("");
                let mut prefix_text = prefix.to_owned();
                if let Some(carry) = prefix_carry.take() {
                    prefix_text = format!("{carry};{prefix}");
                }
                items.push((
                    key,
                    parse_inlines(prefix_text.trim(), ext, notes),
                    parse_inlines(suffix.trim_end(), ext, notes),
                ));
            }
            None => {
                if items.is_empty() {
                    prefix_carry = Some(chunk);
                } else {
                    trailing_suffix = Some(chunk);
                }
            }
        }
    }

    if items.is_empty() {
        return None;
    }

    if let (Some(suffix), Some(last)) = (trailing_suffix, items.last_mut()) {
        let mut combined = last.2.clone();
        if !combined.is_empty() {
            combined.push(Inline::Str(";".into()));
        }
        combined.extend(parse_inlines(suffix.trim(), ext, notes));
        last.2 = combined;
    }

    let citations = items
        .into_iter()
        .enumerate()
        .map(|(idx, (id, prefix, suffix))| carta_ast::Citation {
            id: id.into(),
            prefix,
            suffix,
            mode: if idx == 0 {
                mode.clone()
            } else {
                carta_ast::CitationMode::NormalCitation
            },
            note_num: 0,
            hash: 0,
        })
        .collect();
    Some(citations)
}

fn citation_mode(style: Option<&str>) -> carta_ast::CitationMode {
    match style {
        Some("t" | "text" | "author") => carta_ast::CitationMode::AuthorInText,
        Some(s)
            if s.starts_with("na") || s.starts_with("noauthor") || s.starts_with("suppress") =>
        {
            carta_ast::CitationMode::SuppressAuthor
        }
        _ => carta_ast::CitationMode::NormalCitation,
    }
}

fn is_citation_key_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ':' | '.' | '/')
}
