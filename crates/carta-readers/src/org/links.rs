//! Document-wide lookup of named inline targets.

use std::collections::BTreeMap;
use std::mem;

use carta_ast::{Attr, Block, Document, Inline, Text};
use carta_core::walk::{for_each_block, for_each_document_inline};

use crate::heading_ids::IdRegistry;

/// A private prefix for unresolved names; NUL cannot start a URI.
pub(super) const INTERNAL_TARGET: &str = "\0org-target:";

/// Resolve named links after all inline targets are known.
pub(super) fn resolve_links(document: &mut Document) {
    let mut identifiers = IdRegistry::default();
    for_each_block(&mut document.blocks, &mut |block| {
        let attributes = match &*block {
            Block::Header(_, attributes, _)
            | Block::CodeBlock(attributes, _)
            | Block::Div(attributes, _)
            | Block::Figure(attributes, _, _) => Some(attributes.as_ref()),
            Block::Table(table) => Some(&table.attr),
            _ => None,
        };
        if let Some(attributes) = attributes
            && !attributes.id.is_empty()
        {
            identifiers.reserve_native(&attributes.id);
        }
    });

    let mut targets = BTreeMap::new();
    for_each_document_inline(document, &mut |inline| {
        if let Inline::Span(attributes, _) = inline
            && !attributes.id.is_empty()
        {
            let name = mem::take(&mut attributes.id);
            let identifier = name
                .chars()
                .map(|character| {
                    if character.is_alphanumeric() || matches!(character, '_' | '-' | '.' | ':') {
                        character
                    } else {
                        '-'
                    }
                })
                .collect();
            attributes.id = identifiers.assign_native(identifier).into();
            targets.entry(name).or_insert_with(|| attributes.id.clone());
        }
    });
    for_each_document_inline(document, &mut |inline| {
        let Inline::Link(_, label, target) = inline else {
            return;
        };
        let Some(name) = target.url.strip_prefix(INTERNAL_TARGET) else {
            return;
        };
        if let Some(identifier) = targets.get(name) {
            target.url = format!("#{identifier}").into();
            return;
        }
        let name = Text::from(name);
        let label = mem::take(label);
        *inline = Inline::Span(
            Box::new(Attr {
                classes: vec!["spurious-link".into()],
                attributes: vec![("target".into(), name)],
                ..Attr::default()
            }),
            vec![Inline::Emph(label)],
        );
    });
}
