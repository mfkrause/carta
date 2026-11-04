#![allow(
    clippy::indexing_slicing,
    reason = "The block scanner supplies bounded line positions."
)]

use super::block::physical_blank_line;
use super::{Attributes, Parser, class_attr};
use carta_ast::{Block, Inline, ListAttributes, ListNumberDelim, ListNumberStyle};

impl Parser {
    fn item_blocks(&mut self, lines: &[&str], cursor: &mut usize) -> Vec<Block> {
        if self.depth >= 128 {
            self.syntax_error("block nesting limit exceeded");
            return vec![];
        }
        self.depth += 1;
        let blocks = self.scan_blocks(lines, cursor, true, None, false, '\0');
        self.depth -= 1;
        blocks
    }

    pub(super) fn definition_list(
        &mut self,
        lines: &[&str],
        position: &mut usize,
        ordered: bool,
    ) -> Block {
        let count = definition_depth(lines[*position]);
        let mut entries = Vec::new();
        let mut line_override = None;
        while let Some(line) = line_override
            .take()
            .or_else(|| lines.get(*position).map(|line| (*line).to_owned()))
        {
            let Some((term, content)) = definition(&line) else {
                break;
            };
            if definition_depth(&line) != count {
                break;
            }
            let term = self.definition_term(super::trim_space(term));
            let mut body = Vec::new();
            if !content.is_empty() {
                body.push(content);
            }
            body.extend_from_slice(&lines[*position + 1..]);
            if let Some(first) = body.first_mut() {
                *first = first.trim_start_matches(' ');
            }
            self.list_context.push((':', count - 1));
            let mut consumed = 0;
            let blocks = self.item_blocks(&body, &mut consumed);
            self.list_context.pop();
            self.list_boundary = false;

            if self
                .pending_line
                .as_deref()
                .is_some_and(|line| definition(line).is_some() && definition_depth(line) == count)
            {
                *position += if content.is_empty() {
                    consumed + 1
                } else {
                    consumed
                };
                entries.push((term, vec![blocks]));
                line_override = self.pending_line.take();
                continue;
            }

            if consumed == 0
                && self.pending_line.is_none()
                && definition(content).is_some_and(|_| definition_depth(content) == count)
            {
                entries.push((term, vec![blocks]));
                line_override = Some(content.to_owned());
                continue;
            }

            line_override = None;
            *position += if self.pending_line.is_some() {
                consumed
            } else if content.is_empty() {
                consumed + 1
            } else {
                consumed.max(1)
            };
            entries.push((term, vec![blocks]));
            if self.pending_line.is_some() {
                break;
            }
            while lines
                .get(*position)
                .is_some_and(|line| physical_blank_line(line) || line.starts_with("//"))
            {
                *position += 1;
            }
        }
        if ordered {
            Block::OrderedList(
                ListAttributes {
                    start: 1,
                    style: ListNumberStyle::DefaultStyle,
                    delim: ListNumberDelim::DefaultDelim,
                },
                entries
                    .into_iter()
                    .map(|entry| vec![Block::DefinitionList(vec![entry])])
                    .collect(),
            )
        } else {
            Block::DefinitionList(entries)
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "The loop shares virtual line cursors and list state."
    )]
    pub(super) fn list(
        &mut self,
        lines: &[&str],
        position: &mut usize,
        attributes: &Attributes,
    ) -> Block {
        let initial = if self.list_context.is_empty() {
            marker(lines[*position])
        } else {
            marker_in_list(lines[*position])
        }
        .unwrap_or_default();
        self.list_context.push((initial.kind, initial.depth));
        let mut items = Vec::new();
        let mut line_override = None;
        loop {
            let current_line = line_override
                .take()
                .or_else(|| lines.get(*position).map(|line| (*line).to_owned()));
            let Some(current_line) = current_line else {
                break;
            };
            let Some(current) = marker_in_list(&current_line) else {
                break;
            };
            if current.kind != initial.kind || current.depth != initial.depth {
                break;
            }
            let first_item = items.is_empty();
            let mut content = current.content.trim_start_matches(' ');
            let start_checkbox = if first_item && matches!(current.kind, '*' | '-') {
                checkbox_prefix(content)
            } else {
                None
            };
            if let Some((_, after_checkbox)) = start_checkbox {
                content = after_checkbox;
            }
            let item_checkbox = checkbox_prefix(content);
            if let Some((_, after_checkbox)) = item_checkbox {
                content = after_checkbox;
            }
            let check = if first_item {
                start_checkbox.map(|(check, _)| check)
            } else {
                item_checkbox.map(|(check, _)| check)
            };
            let mut body = vec![content];
            body.extend_from_slice(&lines[*position + 1..]);
            let mut consumed = 0;
            let mut blocks = self.item_blocks(&body, &mut consumed);
            if let Some(check) = check {
                match blocks.first_mut() {
                    Some(Block::Para(nodes) | Block::Plain(nodes)) => {
                        nodes.insert(0, Inline::Space);
                        nodes.insert(0, Inline::Str(check.into()));
                    }
                    _ => blocks.insert(0, Block::Para(vec![Inline::Str(check.into())])),
                }
            }
            let pending_list_item = self
                .pending_line
                .as_deref()
                .and_then(marker_in_list)
                .is_some_and(|marker| marker.kind == initial.kind && marker.depth == initial.depth);
            let item_marker = marker_in_list(content);
            let virtual_item = consumed == 0
                && self.pending_line.is_none()
                && !self.list_boundary
                && item_marker.as_ref().is_some_and(|marker| {
                    marker.kind == initial.kind && marker.depth == initial.depth
                });
            let bubble_item = consumed == 0
                && self.pending_line.is_none()
                && !self.list_boundary
                && (item_marker.as_ref().is_some_and(|marker| {
                    marker.kind == initial.kind && marker.depth < initial.depth
                }) || marker_after_space(content).is_some());
            items.push(blocks);
            if virtual_item {
                line_override = Some(content.to_owned());
                continue;
            }
            if bubble_item {
                self.pending_line = Some(content.to_owned());
                break;
            }
            *position += if self.pending_line.is_some() {
                consumed
            } else {
                consumed.max(1)
            };
            if pending_list_item {
                line_override = self.pending_line.take();
                continue;
            }
            if self.list_boundary || self.pending_line.is_some() {
                break;
            }
        }
        self.list_context.pop();
        if matches!(initial.kind, '*' | '-') {
            Block::BulletList(items)
        } else {
            let style = match attributes.style() {
                "arabic" | "decimal" => ListNumberStyle::Decimal,
                "loweralpha" => ListNumberStyle::LowerAlpha,
                "upperalpha" => ListNumberStyle::UpperAlpha,
                "lowerroman" => ListNumberStyle::LowerRoman,
                "upperroman" => ListNumberStyle::UpperRoman,
                _ => ListNumberStyle::DefaultStyle,
            };
            let list = Block::OrderedList(
                ListAttributes {
                    start: initial
                        .start
                        .or_else(|| attributes.get("start").and_then(|s| s.parse().ok()))
                        .unwrap_or(1),
                    style,
                    delim: ListNumberDelim::DefaultDelim,
                },
                items,
            );
            if initial.kind == '<' {
                Block::Div(Box::new(class_attr("callout-list")), vec![list])
            } else {
                list
            }
        }
    }
}

fn checkbox_prefix(content: &str) -> Option<(&'static str, &str)> {
    let content = content.trim_start_matches(' ');
    [("[*] ", "☒"), ("[x] ", "☒"), ("[ ] ", "☐")]
        .into_iter()
        .find_map(|(prefix, check)| {
            content
                .strip_prefix(prefix)
                .map(|after| (check, after.trim_start_matches(' ')))
        })
}

#[derive(Default)]
pub(super) struct Marker<'a> {
    pub(super) kind: char,
    pub(super) depth: usize,
    pub(super) start: Option<i64>,
    pub(super) content: &'a str,
}
pub(super) fn marker(line: &str) -> Option<Marker<'_>> {
    let line_with_space = line.trim_start_matches(' ');
    let first = line_with_space.chars().next()?;
    if matches!(first, '*' | '.' | '-') {
        let count = line_with_space.chars().take_while(|c| *c == first).count();
        let rest = line_with_space.get(count..)?.strip_prefix(' ')?;
        return Some(Marker {
            kind: first,
            depth: count,
            start: None,
            content: rest,
        });
    }
    if line.chars().next()?.is_ascii_digit() {
        let end = line.find(". ")?;
        if !line[..end].chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        return Some(Marker {
            kind: '1',
            depth: 1,
            start: Some(line[..end].bytes().fold(0i64, |value, digit| {
                value.wrapping_mul(10).wrapping_add(i64::from(digit - b'0'))
            })),
            content: &line[end + 2..],
        });
    }
    if first == '<' {
        let end = line_with_space.find("> ")?;
        let label = &line_with_space[1..end];
        if label != "." && !label.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        return Some(Marker {
            kind: '<',
            depth: 1,
            start: None,
            content: &line_with_space[end + 2..],
        });
    }
    None
}

pub(super) fn marker_in_list(line: &str) -> Option<Marker<'_>> {
    marker(line.trim_start_matches(' '))
}
pub(super) fn marker_after_space(line: &str) -> Option<Marker<'_>> {
    let after_space = super::trim_space_start(line);
    (after_space.len() < line.len())
        .then(|| marker_in_list(super::trim_space(after_space)))
        .flatten()
}
pub(super) fn definition(line: &str) -> Option<(&str, &str)> {
    for delimiter in ["::::", ":::", "::"] {
        if let Some((term, rest)) = line.split_once(delimiter) {
            return Some((term, rest.trim_start_matches(' ')));
        }
    }
    None
}
pub(super) fn definition_depth(line: &str) -> usize {
    line.find("::").map_or(0, |position| {
        line[position..].chars().take_while(|c| *c == ':').count()
    })
}

pub(super) fn definition_item(line: &str, context: Option<(char, usize)>) -> bool {
    let expected_depth = context
        .filter(|(kind, _)| *kind == ':')
        .map_or(2, |(_, depth)| depth.saturating_add(2));
    definition(line).is_some() && definition_depth(line) == expected_depth
}
