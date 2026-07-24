//! Bullet and ordered list parsing, nesting by indentation.

use carta_ast::{Block, ListAttributes, ListNumberDelim, ListNumberStyle};

use super::helpers::leading_spaces;
use super::inline::inline_content;
use super::{Ctx, MAX_DEPTH};

/// The list marker on a line: its indentation and whether it is ordered (`-`) rather than a bullet
/// (`*`). A marker needs at least two leading spaces and a space after the marker character.
fn list_marker(line: &str) -> Option<(usize, bool)> {
    let indent = leading_spaces(line);
    if indent < 2 {
        return None;
    }
    let chars: Vec<char> = line.chars().collect();
    let marker = chars.get(indent)?;
    let ordered = match marker {
        '*' => false,
        '-' => true,
        _ => return None,
    };
    if chars.get(indent + 1) == Some(&' ') {
        Some((indent, ordered))
    } else {
        None
    }
}

/// The nesting level of a list line: one level for every two columns of indentation, so indents of
/// two and three columns share level one, four and five share level two, and so on.
fn list_level(indent: usize) -> usize {
    indent / 2
}

/// Whether a line opens a list: it carries a list marker that sits at the top level (level one).
/// A marker indented deeper than that does not begin a list and is left to become indented code.
pub(super) fn opens_list(line: &str) -> bool {
    list_marker(line).is_some_and(|(indent, _)| list_level(indent) == 1)
}

/// Parse a thematically grouped run of list lines into one bullet or ordered list.
pub(super) fn parse_list(lines: &[&str], index: &mut usize, ctx: Ctx, depth: usize) -> Block {
    let start = *index;
    let mut items = Vec::new();
    while *index < lines.len() {
        let line = lines.get(*index).copied().unwrap_or("");
        let Some((indent, ordered)) = list_marker(line) else {
            break;
        };
        let text: String = line.chars().skip(indent + 2).collect();
        items.push((list_level(indent), ordered, text));
        *index += 1;
    }
    // A level jump of more than one ends the list (rest parsed afresh); an over-indented marker
    // becomes indented code.
    let cutoff = list_cutoff(&items);
    let consumed = items.get(..cutoff).unwrap_or(&[]);
    let mut pos = 0;
    let list = build_list(consumed, &mut pos, ctx, depth);
    // A marker-type switch or dedent below the opening level ends the list; rewind for a fresh parse.
    *index = start + pos;
    list
}

/// The number of leading items that form one list: the run ends at the first item whose level rises
/// more than one above the item before it.
fn list_cutoff(items: &[(usize, bool, String)]) -> usize {
    let mut previous = None;
    for (i, (level, _, _)) in items.iter().enumerate() {
        if let Some(prev) = previous
            && *level > prev + 1
        {
            return i;
        }
        previous = Some(*level);
    }
    items.len()
}

/// Build one list (and its nested sublists) from the collected items, advancing `pos`. A deeper
/// level opens a child list; the same level with the other marker ends this list.
fn build_list(items: &[(usize, bool, String)], pos: &mut usize, ctx: Ctx, depth: usize) -> Block {
    let (base_level, ordered) = items
        .get(*pos)
        .map_or((0, false), |(level, ordered, _)| (*level, *ordered));
    let mut entries: Vec<Vec<Block>> = Vec::new();
    while let Some((level, item_ordered, text)) = items.get(*pos) {
        if *level < base_level {
            break;
        }
        if *level == base_level {
            if *item_ordered != ordered {
                break;
            }
            let mut blocks = vec![Block::Plain(inline_content(text, ctx, depth))];
            *pos += 1;
            if depth < MAX_DEPTH && items.get(*pos).is_some_and(|(l, _, _)| *l > base_level) {
                blocks.push(build_list(items, pos, ctx, depth + 1));
            }
            entries.push(blocks);
        } else if depth < MAX_DEPTH {
            let child = build_list(items, pos, ctx, depth + 1);
            match entries.last_mut() {
                Some(last) => last.push(child),
                None => entries.push(vec![child]),
            }
        } else {
            *pos += 1;
        }
    }
    if ordered {
        Block::OrderedList(
            ListAttributes {
                start: 1,
                style: ListNumberStyle::DefaultStyle,
                delim: ListNumberDelim::DefaultDelim,
            },
            entries,
        )
    } else {
        Block::BulletList(entries)
    }
}
