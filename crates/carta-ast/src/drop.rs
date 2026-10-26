//! Release tree children without recursive drop.

use crate::{Block, Caption, Inline, MetaValue, Row, Text};
use std::collections::BTreeMap;

enum Children {
    Blocks(Vec<Block>),
    Inlines(Vec<Inline>),
    Metadata(Vec<MetaValue>),
    Map(BTreeMap<Text, MetaValue>),
}

trait TakeChildren {
    fn take_children(&mut self, pending: &mut Vec<Children>);
}

fn blocks(values: &mut Vec<Block>, pending: &mut Vec<Children>) {
    if !values.is_empty() {
        pending.push(Children::Blocks(std::mem::take(values)));
    }
}

fn inlines(values: &mut Vec<Inline>, pending: &mut Vec<Children>) {
    if !values.is_empty() {
        pending.push(Children::Inlines(std::mem::take(values)));
    }
}

fn caption(value: &mut Caption, pending: &mut Vec<Children>) {
    if let Some(short) = &mut value.short {
        inlines(short, pending);
    }
    blocks(&mut value.long, pending);
}

fn rows(values: &mut [Row], pending: &mut Vec<Children>) {
    for row in values {
        for cell in &mut row.cells {
            blocks(&mut cell.content, pending);
        }
    }
}

impl TakeChildren for Block {
    fn take_children(&mut self, pending: &mut Vec<Children>) {
        match self {
            Self::Plain(values) | Self::Para(values) | Self::Header(_, _, values) => {
                inlines(values, pending);
            }
            Self::LineBlock(lines) => {
                for line in lines {
                    inlines(line, pending);
                }
            }
            Self::BlockQuote(values) | Self::Div(_, values) => blocks(values, pending),
            Self::OrderedList(_, items) | Self::BulletList(items) => {
                for item in items {
                    blocks(item, pending);
                }
            }
            Self::DefinitionList(entries) => {
                for (term, definitions) in entries {
                    inlines(term, pending);
                    for definition in definitions {
                        blocks(definition, pending);
                    }
                }
            }
            Self::Table(table) => {
                caption(&mut table.caption, pending);
                rows(&mut table.head.rows, pending);
                for body in &mut table.bodies {
                    rows(&mut body.head, pending);
                    rows(&mut body.body, pending);
                }
                rows(&mut table.foot.rows, pending);
            }
            Self::Figure(_, value, values) => {
                caption(value, pending);
                blocks(values, pending);
            }
            Self::CodeBlock(..) | Self::RawBlock(..) | Self::HorizontalRule => {}
        }
    }
}

impl TakeChildren for Inline {
    fn take_children(&mut self, pending: &mut Vec<Children>) {
        match self {
            Self::Emph(values)
            | Self::Underline(values)
            | Self::Strong(values)
            | Self::Strikeout(values)
            | Self::Superscript(values)
            | Self::Subscript(values)
            | Self::SmallCaps(values)
            | Self::Quoted(_, values)
            | Self::Link(_, values, _)
            | Self::Image(_, values, _)
            | Self::Span(_, values) => inlines(values, pending),
            Self::Cite(citations, values) => {
                for citation in citations {
                    inlines(&mut citation.prefix, pending);
                    inlines(&mut citation.suffix, pending);
                }
                inlines(values, pending);
            }
            Self::Note(values) => blocks(values, pending),
            Self::Str(_)
            | Self::Code(..)
            | Self::Space
            | Self::SoftBreak
            | Self::LineBreak
            | Self::Math(..)
            | Self::RawInline(..) => {}
        }
    }
}

impl TakeChildren for MetaValue {
    fn take_children(&mut self, pending: &mut Vec<Children>) {
        match self {
            Self::MetaMap(values) if !values.is_empty() => {
                pending.push(Children::Map(std::mem::take(values)));
            }
            Self::MetaList(values) if !values.is_empty() => {
                pending.push(Children::Metadata(std::mem::take(values)));
            }
            Self::MetaInlines(values) => inlines(values, pending),
            Self::MetaBlocks(values) => blocks(values, pending),
            Self::MetaMap(_) | Self::MetaList(_) | Self::MetaBool(_) | Self::MetaString(_) => {}
        }
    }
}

fn drop_children(value: &mut impl TakeChildren) {
    let mut pending = Vec::new();
    value.take_children(&mut pending);
    while let Some(children) = pending.pop() {
        match children {
            Children::Blocks(mut values) => {
                for value in &mut values {
                    value.take_children(&mut pending);
                }
            }
            Children::Inlines(mut values) => {
                for value in &mut values {
                    value.take_children(&mut pending);
                }
            }
            Children::Metadata(mut values) => {
                for value in &mut values {
                    value.take_children(&mut pending);
                }
            }
            Children::Map(mut values) => {
                for value in values.values_mut() {
                    value.take_children(&mut pending);
                }
            }
        }
    }
}

impl Drop for Block {
    fn drop(&mut self) {
        drop_children(self);
    }
}

impl Drop for Inline {
    fn drop(&mut self) {
        drop_children(self);
    }
}

impl Drop for MetaValue {
    fn drop(&mut self) {
        drop_children(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_trees_can_be_cloned_compared_and_dropped_on_a_small_stack() {
        std::thread::Builder::new()
            .stack_size(128 * 1024)
            .spawn(|| {
                let mut block = Block::Para(vec![Inline::Str("leaf".into())]);
                for _ in 0..20_000 {
                    block = Block::BlockQuote(vec![block]);
                }
                let mut value = MetaValue::MetaBlocks(vec![block]);
                for _ in 0..10_000 {
                    value = MetaValue::MetaMap(BTreeMap::from([(
                        "child".into(),
                        MetaValue::MetaList(vec![value]),
                    )]));
                }
                assert_eq!(value, value.clone());
            })
            .expect("spawn caller")
            .join()
            .expect("caller finished");
    }
}
