//! Keyword lines (`#+key: value`): metadata, affiliated captions, and header includes.

use std::collections::BTreeMap;

use carta_ast::{Block, Format, Inline, MetaValue, Text};
use carta_core::Extensions;

use super::blocks::Affiliated;
use super::inline::parse_inlines;
use super::starts_with_ci;

/// Splits a `#+key: value` keyword line into `(key, value)`. Block delimiters (`#+begin_…`) are not
/// keyword lines.
pub(super) fn keyword_line(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix("#+")?;
    let colon = rest.find(':')?;
    let key = rest.get(..colon)?;
    if key.is_empty()
        || !key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
    {
        return None;
    }
    if key.eq_ignore_ascii_case("begin_src")
        || starts_with_ci(key, "begin_")
        || starts_with_ci(key, "end_")
    {
        return None;
    }
    let value = rest.get(colon + 1..).unwrap_or("").trim_start().to_owned();
    Some((key.to_owned(), value))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn handle_keyword(
    key: &str,
    value: &str,
    line: &str,
    ext: Extensions,
    notes: &BTreeMap<String, Vec<Block>>,
    meta: &mut BTreeMap<Text, MetaValue>,
    pending: &mut Affiliated,
    out: &mut Vec<Block>,
) {
    let upper = key.to_ascii_uppercase();
    match upper.as_str() {
        "TITLE" | "SUBTITLE" | "AUTHOR" | "DATE" | "KEYWORDS" | "DESCRIPTION" => {
            meta.insert(
                upper.to_ascii_lowercase().into(),
                MetaValue::MetaInlines(parse_inlines(value, ext, notes)),
            );
        }
        "LANGUAGE" => {
            meta.insert("lang".into(), MetaValue::MetaString(value.into()));
        }
        "CAPTION" => pending.caption = Some(parse_inlines(value, ext, notes)),
        "NAME" | "LABEL" => pending.name = Some(value.to_owned()),
        "OPTIONS" | "TODO" | "SEQ_TODO" | "TYP_TODO" | "PRIORITIES" | "TAGS" | "COLUMNS"
        | "SETUPFILE" | "CONSTANTS" | "MACRO" | "DRAWERS" | "ARCHIVE" | "RESULTS" | "HEADER"
        | "PLOT" => {}
        other if other.starts_with("ATTR_") => {}
        other if other.starts_with("LATEX_HEADER") => {
            append_header_include(meta, "latex", value);
        }
        other if other.starts_with("HTML_HEAD") => {
            append_header_include(meta, "html", value);
        }
        _ => out.push(Block::RawBlock(
            Format("org".into()),
            line.trim_end().into(),
        )),
    }
}

fn append_header_include(meta: &mut BTreeMap<Text, MetaValue>, format: &str, value: &str) {
    let entry =
        MetaValue::MetaInlines(vec![Inline::RawInline(Format(format.into()), value.into())]);
    match meta
        .entry("header-includes".into())
        .or_insert_with(|| MetaValue::MetaList(Vec::new()))
    {
        MetaValue::MetaList(list) => list.push(entry),
        slot => *slot = MetaValue::MetaList(vec![entry]),
    }
}
