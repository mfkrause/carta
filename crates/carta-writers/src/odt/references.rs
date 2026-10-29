//! Known heading and caption targets for document reference fields.

use std::collections::BTreeMap;

use carta_ast::{Block, Inline, Text};

#[derive(Clone, Copy)]
pub(super) enum ReferenceKind {
    Bookmark,
    Sequence,
}

enum Node<'a> {
    Block(&'a Block),
    Inline(&'a Inline),
}

pub(super) fn collect(blocks: &[Block]) -> BTreeMap<Text, ReferenceKind> {
    let mut references = BTreeMap::new();
    let mut pending: Vec<_> = blocks.iter().rev().map(Node::Block).collect();
    while let Some(node) = pending.pop() {
        match node {
            Node::Block(block) => {
                let target = match block {
                    Block::Header(_, attr, _) => Some((&attr.id, ReferenceKind::Bookmark)),
                    Block::Figure(attr, _, _) => Some((&attr.id, ReferenceKind::Sequence)),
                    Block::Table(table) => Some((&table.attr.id, ReferenceKind::Sequence)),
                    _ => None,
                };
                if let Some((identifier, kind)) = target
                    && !identifier.is_empty()
                {
                    references.entry(identifier.clone()).or_insert(kind);
                }
                push_block_children(block, &mut pending);
            }
            Node::Inline(inline) => match inline {
                Inline::Note(blocks) => pending.extend(blocks.iter().rev().map(Node::Block)),
                Inline::Cite(citations, inner) => {
                    pending.extend(inner.iter().rev().map(Node::Inline));
                    for citation in citations.iter().rev() {
                        pending.extend(citation.suffix.iter().rev().map(Node::Inline));
                        pending.extend(citation.prefix.iter().rev().map(Node::Inline));
                    }
                }
                Inline::Emph(inner)
                | Inline::Underline(inner)
                | Inline::Strong(inner)
                | Inline::Strikeout(inner)
                | Inline::Superscript(inner)
                | Inline::Subscript(inner)
                | Inline::SmallCaps(inner)
                | Inline::Quoted(_, inner)
                | Inline::Link(_, inner, _)
                | Inline::Image(_, inner, _)
                | Inline::Span(_, inner) => pending.extend(inner.iter().rev().map(Node::Inline)),
                Inline::Str(_)
                | Inline::Space
                | Inline::SoftBreak
                | Inline::LineBreak
                | Inline::Code(..)
                | Inline::Math(..)
                | Inline::RawInline(..) => {}
            },
        }
    }
    references
}

fn push_block_children<'a>(block: &'a Block, pending: &mut Vec<Node<'a>>) {
    match block {
        Block::Plain(inlines) | Block::Para(inlines) | Block::Header(_, _, inlines) => {
            pending.extend(inlines.iter().rev().map(Node::Inline));
        }
        Block::LineBlock(lines) => {
            pending.extend(lines.iter().flatten().rev().map(Node::Inline));
        }
        Block::BlockQuote(inner) | Block::Div(_, inner) => {
            pending.extend(inner.iter().rev().map(Node::Block));
        }
        Block::BulletList(items) | Block::OrderedList(_, items) => {
            for item in items.iter().rev() {
                pending.extend(item.iter().rev().map(Node::Block));
            }
        }
        Block::DefinitionList(items) => {
            for (term, definitions) in items.iter().rev() {
                for definition in definitions.iter().rev() {
                    pending.extend(definition.iter().rev().map(Node::Block));
                }
                pending.extend(term.iter().rev().map(Node::Inline));
            }
        }
        Block::Figure(_, caption, inner) => {
            pending.extend(inner.iter().rev().map(Node::Block));
            pending.extend(caption.long.iter().rev().map(Node::Block));
            if let Some(short) = &caption.short {
                pending.extend(short.iter().rev().map(Node::Inline));
            }
        }
        Block::Table(table) => {
            for row in table
                .head
                .rows
                .iter()
                .chain(crate::common::body_rows(table))
                .chain(&table.foot.rows)
                .rev()
            {
                for cell in row.cells.iter().rev() {
                    pending.extend(cell.content.iter().rev().map(Node::Block));
                }
            }
            pending.extend(table.caption.long.iter().rev().map(Node::Block));
            if let Some(short) = &table.caption.short {
                pending.extend(short.iter().rev().map(Node::Inline));
            }
        }
        Block::CodeBlock(..) | Block::RawBlock(..) | Block::HorizontalRule => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use carta_ast::{Attr, Caption};

    fn heading(identifier: &str) -> Block {
        Block::Header(
            1,
            Box::new(Attr {
                id: identifier.into(),
                ..Default::default()
            }),
            Vec::new(),
        )
    }

    #[test]
    fn the_first_target_keeps_its_kind_across_nested_nodes() {
        let blocks = vec![
            Block::LineBlock(vec![
                vec![Inline::Note(vec![heading("duplicate")])],
                vec![Inline::Note(vec![Block::Figure(
                    Box::new(Attr {
                        id: "duplicate".into(),
                        ..Default::default()
                    }),
                    Box::new(Caption::default()),
                    Vec::new(),
                )])],
            ]),
            Block::Table(Box::new(carta_ast::Table {
                attr: Attr {
                    id: "table".into(),
                    ..Default::default()
                },
                ..Default::default()
            })),
        ];
        let references = collect(&blocks);
        assert!(matches!(
            references.get("duplicate"),
            Some(ReferenceKind::Bookmark)
        ));
        assert!(matches!(
            references.get("table"),
            Some(ReferenceKind::Sequence)
        ));
    }

    #[test]
    fn citation_prefixes_and_suffixes_can_hold_reference_targets() {
        let blocks = vec![Block::Para(vec![Inline::Cite(
            vec![carta_ast::Citation {
                id: "item".into(),
                prefix: vec![Inline::Note(vec![heading("prefix")])],
                suffix: vec![Inline::Note(vec![heading("suffix")])],
                mode: carta_ast::CitationMode::NormalCitation,
                note_num: 0,
                hash: 0,
            }],
            Vec::new(),
        )])];
        let references = collect(&blocks);
        for identifier in ["prefix", "suffix"] {
            assert!(matches!(
                references.get(identifier),
                Some(ReferenceKind::Bookmark)
            ));
        }
    }

    #[test]
    fn deeply_nested_targets_do_not_use_the_call_stack() {
        let mut block = heading("deep");
        for _ in 0..10_000 {
            block = Block::Div(Box::default(), vec![block]);
        }
        let references = collect(&[block]);
        assert!(matches!(
            references.get("deep"),
            Some(ReferenceKind::Bookmark)
        ));
    }
}
