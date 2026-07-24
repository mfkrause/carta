//! Parsing of bullet, ordered, and definition lists.

use carta_ast::{Block, Inline, ListAttributes, ListNumberDelim, ListNumberStyle};

use super::blocks::is_list_marker;
use super::links::{bare_url, plain_or_figure};
use super::{MAX_BLOCK_DEPTH, Parser, at, collect_range, line_end, skip_construct};

/// One line of list markup: its leading marker run and the trimmed text that follows.
struct ListItem {
    markers: Vec<char>,
    content: String,
}

/// The list family a marker character opens.
#[derive(PartialEq, Eq, Clone, Copy)]
enum ListKind {
    Bullet,
    Ordered,
    Definition,
}

impl Parser {
    pub(super) fn parse_list(&mut self, chars: &[char], pos: usize) -> (Vec<Block>, usize) {
        let mut items: Vec<ListItem> = Vec::new();
        let mut cursor = pos;
        let n = chars.len();
        while at(chars, cursor).is_some_and(is_list_marker) {
            let le = line_end(chars, cursor);
            let mut scan = cursor;
            let mut markers: Vec<char> = Vec::new();
            while scan < le && at(chars, scan).is_some_and(is_list_marker) {
                if let Some(marker) = at(chars, scan) {
                    markers.push(marker);
                }
                scan += 1;
            }
            let content = collect_range(chars, scan, le).trim().to_string();
            items.push(ListItem { markers, content });
            if le >= n {
                cursor = le;
                break;
            }
            cursor = le + 1;
        }
        (self.build_lists(&items, 0), cursor)
    }

    fn build_lists(&mut self, items: &[ListItem], level: usize) -> Vec<Block> {
        if level >= MAX_BLOCK_DEPTH {
            // Past the nesting cap, item text becomes flat plain blocks so deep marker runs cannot exhaust the stack.
            let mut out: Vec<Block> = Vec::new();
            for item in items {
                let inlines = self.parse_inlines(&item.content);
                if !inlines.is_empty() {
                    out.push(Block::Plain(inlines));
                }
            }
            return out;
        }
        let mut out: Vec<Block> = Vec::new();
        let mut i = 0;
        while i < items.len() {
            let kind = if let Some(&m) = items.get(i).and_then(|it| it.markers.get(level)) {
                list_kind(m)
            } else {
                i += 1;
                continue;
            };
            let mut j = i;
            while j < items.len() {
                match items.get(j).and_then(|it| it.markers.get(level)) {
                    Some(&m) if list_kind(m) == kind => j += 1,
                    _ => break,
                }
            }
            let group = items.get(i..j).unwrap_or(&[]);
            match kind {
                ListKind::Bullet => out.push(Block::BulletList(self.build_simple(group, level))),
                ListKind::Ordered => {
                    out.push(Block::OrderedList(
                        default_list_attrs(),
                        self.build_simple(group, level),
                    ));
                }
                ListKind::Definition => out.push(self.build_definition(group, level)),
            }
            i = j;
        }
        out
    }

    fn build_simple(&mut self, group: &[ListItem], level: usize) -> Vec<Vec<Block>> {
        let mut entries: Vec<Vec<Block>> = Vec::new();
        let mut i = 0;
        while i < group.len() {
            let depth = group.get(i).map_or(0, |it| it.markers.len());
            if depth == level + 1 {
                let content = group.get(i).map_or("", |it| it.content.as_str());
                let mut blocks = vec![plain_or_figure(self.parse_inlines(content))];
                i += 1;
                let start = i;
                while i < group.len() && group.get(i).map_or(0, |it| it.markers.len()) > level + 1 {
                    i += 1;
                }
                if let Some(sub) = group.get(start..i)
                    && !sub.is_empty()
                {
                    blocks.extend(self.build_lists(sub, level + 1));
                }
                entries.push(blocks);
            } else {
                let start = i;
                while i < group.len() && group.get(i).map_or(0, |it| it.markers.len()) > level + 1 {
                    i += 1;
                }
                if i == start {
                    i += 1;
                }
                let blocks = group
                    .get(start..i)
                    .map(|sub| self.build_lists(sub, level + 1))
                    .unwrap_or_default();
                entries.push(blocks);
            }
        }
        entries
    }

    fn build_definition(&mut self, group: &[ListItem], level: usize) -> Block {
        let mut pairs: Vec<(Vec<Inline>, Vec<Vec<Block>>)> = Vec::new();
        let mut i = 0;
        while i < group.len() {
            let Some(item) = group.get(i) else { break };
            if item.markers.len() == level + 1 {
                let marker = item.markers.get(level).copied().unwrap_or(':');
                let content = item.content.clone();
                i += 1;
                let start = i;
                while i < group.len() && group.get(i).map_or(0, |it| it.markers.len()) > level + 1 {
                    i += 1;
                }
                let nested = group
                    .get(start..i)
                    .map(|sub| self.build_lists(sub, level + 1))
                    .unwrap_or_default();
                if marker == ';' {
                    let (term_str, def_str) = split_term(&content);
                    let term = self.parse_inlines(&term_str);
                    let mut defs: Vec<Vec<Block>> = Vec::new();
                    if let Some(d) = def_str {
                        defs.push(vec![plain_or_figure(self.parse_inlines(&d))]);
                    }
                    if !nested.is_empty() {
                        match defs.last_mut() {
                            Some(last) => last.extend(nested),
                            None => defs.push(nested),
                        }
                    }
                    // Terms stacked with no definition between them share one entry, separated by line breaks.
                    match pairs.last_mut() {
                        Some((last_term, last_defs)) if last_defs.is_empty() => {
                            last_term.push(Inline::LineBreak);
                            last_term.extend(term);
                            *last_defs = defs;
                        }
                        _ => pairs.push((term, defs)),
                    }
                } else {
                    let mut blocks = vec![plain_or_figure(self.parse_inlines(&content))];
                    blocks.extend(nested);
                    match pairs.last_mut() {
                        Some(last) => last.1.push(blocks),
                        None => pairs.push((Vec::new(), vec![blocks])),
                    }
                }
            } else {
                let start = i;
                while i < group.len() && group.get(i).map_or(0, |it| it.markers.len()) > level + 1 {
                    i += 1;
                }
                if i == start {
                    i += 1;
                }
                let nested = group
                    .get(start..i)
                    .map(|sub| self.build_lists(sub, level + 1))
                    .unwrap_or_default();
                match pairs.last_mut() {
                    Some(last) => match last.1.last_mut() {
                        Some(d) => d.extend(nested),
                        None => last.1.push(nested),
                    },
                    None => pairs.push((Vec::new(), vec![nested])),
                }
            }
        }
        Block::DefinitionList(pairs)
    }
}

/// Splits a definition term at the first top-level `:`, skipping constructs so a `:` inside a link
/// or template is not treated as the separator.
fn split_term(content: &str) -> (String, Option<String>) {
    let chars: Vec<char> = content.chars().collect();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        if let Some(next) = skip_construct(&chars, i)
            && next > i
        {
            i = next;
            continue;
        }
        // A bare URL is stepped over whole so the `:` in its scheme is not read as the separator.
        if let Some((_, next)) = bare_url(&chars, i)
            && next > i
        {
            i = next;
            continue;
        }
        if at(&chars, i) == Some(':') {
            let before = collect_range(&chars, 0, i).trim().to_string();
            let after = collect_range(&chars, i + 1, n).trim().to_string();
            return (before, Some(after));
        }
        i += 1;
    }
    (content.trim().to_string(), None)
}

fn list_kind(marker: char) -> ListKind {
    match marker {
        '#' => ListKind::Ordered,
        ';' | ':' => ListKind::Definition,
        _ => ListKind::Bullet,
    }
}

pub(super) fn default_list_attrs() -> ListAttributes {
    ListAttributes {
        start: 1,
        style: ListNumberStyle::DefaultStyle,
        delim: ListNumberDelim::DefaultDelim,
    }
}
