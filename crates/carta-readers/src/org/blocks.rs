//! Block-level Org parsing: the line dispatcher, headlines, and affiliated keywords.

use std::collections::BTreeMap;
use std::mem;

use carta_ast::{Attr, Block, Caption, Inline, MetaValue, Text};
use carta_core::Extensions;

use crate::heading_ids::IdRegistry;

use super::assign_id;
use super::drawers::{
    collect_drawer, collect_fixed_width, drawer_open, is_fixed_width, is_horizontal_rule,
};
use super::greater::{greater_block_open, parse_greater_block};
use super::inline::parse_inlines;
use super::keyword::{handle_keyword, keyword_line};
use super::lists::{list_marker, parse_list};
use super::tables::{build_table, collect_table, is_table_line};

/// Affiliated keywords (`#+caption:`, `#+name:`) that attach to the block that follows them.
#[derive(Default)]
pub(super) struct Affiliated {
    pub(super) caption: Option<Vec<Inline>>,
    pub(super) name: Option<String>,
}

impl Affiliated {
    fn is_empty(&self) -> bool {
        self.caption.is_none() && self.name.is_none()
    }
}

#[allow(clippy::too_many_lines)]
pub(super) fn parse_blocks(
    lines: &[&str],
    ext: Extensions,
    notes: &BTreeMap<String, Vec<Block>>,
    ids: &mut IdRegistry,
    meta: &mut BTreeMap<Text, MetaValue>,
) -> Vec<Block> {
    let mut out = Vec::new();
    let mut pending = Affiliated::default();
    let mut i = 0;
    while let Some(&line) = lines.get(i) {
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        if let Some(level) = headline_level(line) {
            i += 1;
            let mut id_override = None;
            if let Some((custom_id, skip)) = read_property_drawer(lines, i) {
                id_override = custom_id;
                i += skip;
            }
            out.push(build_headline(line, level, id_override, ext, notes, ids));
            pending = Affiliated::default();
            continue;
        }
        if let Some(name) = greater_block_open(line) {
            let (block, consumed) = parse_greater_block(lines, i, &name, ext, notes, ids, meta);
            i += consumed;
            if let Some(block) = block {
                out.push(apply_affiliated(block, &mut pending));
            }
            continue;
        }
        if let Some((key, value)) = keyword_line(line) {
            handle_keyword(&key, &value, line, ext, notes, meta, &mut pending, &mut out);
            i += 1;
            continue;
        }
        if line.trim_start() == "#" || line.trim_start().starts_with("# ") {
            i += 1;
            continue;
        }
        if is_horizontal_rule(line) {
            out.push(Block::HorizontalRule);
            i += 1;
            pending = Affiliated::default();
            continue;
        }
        if is_fixed_width(line) {
            let (text, consumed) = collect_fixed_width(lines, i);
            out.push(Block::CodeBlock(Box::default(), text.into()));
            i += consumed;
            pending = Affiliated::default();
            continue;
        }
        if let Some(name) = drawer_open(line) {
            let (inner, consumed) = collect_drawer(lines, i);
            i += consumed;
            // Metadata drawers are bookkeeping and elided; other named drawers become divs.
            if name.eq_ignore_ascii_case("PROPERTIES") || name.eq_ignore_ascii_case("LOGBOOK") {
                pending = Affiliated::default();
                continue;
            }
            let body = parse_blocks(&inner, ext, notes, ids, meta);
            let attr = Attr {
                classes: vec![name.into(), "drawer".into()],
                ..Attr::default()
            };
            out.push(Block::Div(Box::new(attr), body));
            pending = Affiliated::default();
            continue;
        }
        if is_table_line(line) {
            let (rows, consumed) = collect_table(lines, i);
            let table = build_table(&rows, ext, notes, &mut pending);
            out.push(table);
            i += consumed;
            continue;
        }
        if list_marker(line).is_some() {
            let (block, consumed) = parse_list(lines, i, ext, notes, ids, meta);
            i += consumed;
            if let Some(block) = block {
                out.push(block);
            }
            pending = Affiliated::default();
            continue;
        }
        // Dispatch proved this line is neither blank nor a block opener; continuation starts next line.
        let start = i;
        i += 1;
        while let Some(&l) = lines.get(i) {
            if l.trim().is_empty() || opens_block(l) {
                break;
            }
            i += 1;
        }
        let text = lines
            .get(start..i)
            .unwrap_or(&[])
            .iter()
            .map(|l| l.trim())
            .collect::<Vec<_>>()
            .join("\n");
        let para = Block::Para(parse_inlines(&text, ext, notes));
        out.push(apply_affiliated(para, &mut pending));
    }
    out
}

/// Whether a line begins a block that interrupts an open paragraph.
fn opens_block(line: &str) -> bool {
    headline_level(line).is_some()
        || greater_block_open(line).is_some()
        || keyword_line(line).is_some()
        || line.trim_start() == "#"
        || line.trim_start().starts_with("# ")
        || is_horizontal_rule(line)
        || is_fixed_width(line)
        || drawer_open(line).is_some()
        || is_table_line(line)
        || list_marker(line).is_some()
}

/// Attaches a pending caption/name to a freshly built block: a caption turns a lone-image paragraph
/// into a figure, and a name supplies its identifier.
fn apply_affiliated(block: Block, pending: &mut Affiliated) -> Block {
    if pending.is_empty() {
        return block;
    }
    let Affiliated { caption, name } = mem::take(pending);
    match block {
        Block::Para(inlines) if is_lone_image(&inlines) => {
            let attr = Attr {
                id: name.unwrap_or_default().into(),
                ..Attr::default()
            };
            let long = caption.map(|c| vec![Block::Plain(c)]).unwrap_or_default();
            Block::Figure(
                Box::new(attr),
                Box::new(Caption { short: None, long }),
                vec![Block::Plain(inlines)],
            )
        }
        Block::CodeBlock(mut attr, text) => {
            if let Some(name) = name {
                attr.id = name.into();
            }
            Block::CodeBlock(attr, text)
        }
        other => other,
    }
}

fn is_lone_image(inlines: &[Inline]) -> bool {
    matches!(inlines, [Inline::Image(..)])
}

/// The headline level (count of leading `*`) when a line is a headline, i.e. one or more `*` at
/// column zero followed by a space.
pub(super) fn headline_level(line: &str) -> Option<usize> {
    let stars = line.len() - line.trim_start_matches('*').len();
    if stars == 0 {
        return None;
    }
    match line.as_bytes().get(stars) {
        Some(b' ') => Some(stars),
        _ => None,
    }
}

/// Builds a `Header`, splitting off a leading todo keyword and trailing tags and deriving an
/// identifier from the remaining title text (or the property drawer's custom id).
fn build_headline(
    line: &str,
    level: usize,
    id_override: Option<String>,
    ext: Extensions,
    notes: &BTreeMap<String, Vec<Block>>,
    ids: &mut IdRegistry,
) -> Block {
    let rest = line.get(level..).unwrap_or("").trim();

    let (todo, rest) = split_todo_keyword(rest);
    let (title_text, tags) = split_tags(rest);

    let title_inlines = parse_inlines(title_text, ext, notes);

    let id = if let Some(custom) = id_override {
        ids.reserve_native(&custom);
        custom
    } else {
        assign_id(ids, &carta_ast::to_plain_text(&title_inlines), ext)
    };

    let mut inlines = Vec::new();
    if let Some(keyword) = todo {
        inlines.push(todo_span(keyword));
        inlines.push(Inline::Space);
    }
    inlines.extend(title_inlines);
    if !tags.is_empty() {
        inlines.push(Inline::Space);
        for (n, tag) in tags.iter().enumerate() {
            if n > 0 {
                inlines.push(Inline::Str("\u{a0}".into()));
            }
            inlines.push(tag_span(tag));
        }
    }

    let attr = Attr {
        id: id.into(),
        ..Attr::default()
    };
    let level = i32::try_from(level).unwrap_or(6).clamp(1, 6);
    Block::Header(level, Box::new(attr), inlines)
}

fn todo_span(keyword: &str) -> Inline {
    let state = if keyword == "DONE" { "done" } else { "todo" };
    let attr = Attr {
        classes: vec![state.into(), keyword.into()],
        ..Attr::default()
    };
    Inline::Span(Box::new(attr), vec![Inline::Str(keyword.into())])
}

fn tag_span(tag: &str) -> Inline {
    let attr = Attr {
        classes: vec!["tag".into()],
        attributes: vec![("tag-name".into(), tag.into())],
        ..Attr::default()
    };
    Inline::Span(
        Box::new(attr),
        vec![Inline::SmallCaps(vec![Inline::Str(tag.into())])],
    )
}

/// Splits a leading `TODO`/`DONE` keyword (which must be followed by a space or end the text) from
/// the headline body.
fn split_todo_keyword(rest: &str) -> (Option<&str>, &str) {
    for keyword in ["TODO", "DONE"] {
        if let Some(after) = rest.strip_prefix(keyword)
            && (after.is_empty() || after.starts_with(' '))
        {
            return (Some(keyword), after.trim_start());
        }
    }
    (None, rest)
}

/// Splits trailing `:tag:tag:` tags from a headline, returning the title text and the tag names.
fn split_tags(rest: &str) -> (&str, Vec<String>) {
    let trimmed = rest.trim_end();
    if !trimmed.ends_with(':') {
        return (rest, Vec::new());
    }
    let Some(space) = trimmed.rfind(char::is_whitespace) else {
        return (rest, Vec::new());
    };
    let candidate = trimmed.get(space + 1..).unwrap_or("");
    if candidate.len() < 2 || !candidate.starts_with(':') || !candidate.ends_with(':') {
        return (rest, Vec::new());
    }
    let inner = &candidate[1..candidate.len() - 1];
    if inner.is_empty()
        || !inner
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '@' | '#' | '%' | ':'))
    {
        return (rest, Vec::new());
    }
    let tags: Vec<String> = inner
        .split(':')
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect();
    if tags.is_empty() {
        return (rest, Vec::new());
    }
    (trimmed.get(..space).unwrap_or("").trim_end(), tags)
}

/// Reads a `:PROPERTIES:`…`:END:` drawer immediately following a headline, returning the custom
/// identifier (if any) and the number of lines consumed. Returns `None` when no drawer follows.
fn read_property_drawer(lines: &[&str], start: usize) -> Option<(Option<String>, usize)> {
    let first = lines.get(start)?;
    if !first.trim().eq_ignore_ascii_case(":PROPERTIES:") {
        return None;
    }
    let mut custom = None;
    let mut i = start + 1;
    while let Some(line) = lines.get(i) {
        let trimmed = line.trim();
        if trimmed.eq_ignore_ascii_case(":END:") {
            return Some((custom, i + 1 - start));
        }
        if let Some(rest) = trimmed.strip_prefix(':')
            && let Some((key, value)) = rest.split_once(':')
            && key.eq_ignore_ascii_case("CUSTOM_ID")
        {
            custom = Some(value.trim().to_owned());
        }
        i += 1;
    }
    // Unterminated drawer: leave the lines to the block parser.
    None
}
