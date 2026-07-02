//! Auto-generated header identifiers.
//!
//! When an auto-identifier toggle is on, a header that carries no explicit identifier receives one
//! derived from its text; an explicit identifier is kept and reserved so later derived identifiers
//! avoid it. One walker threads the disambiguation state through the whole document in reading
//! order, so a header's id depends on every header before it. The derivation and disambiguation
//! rules live in [`crate::heading_ids`].

use carta_ast::{Block, Inline, Table, to_plain_text};
use carta_core::Extensions;

use crate::heading_ids::{IdRegistry, IdScheme};

/// Fill in empty header identifiers across the document in reading order. `markdown` selects the
/// broad Markdown dialect, where `auto_identifiers` is the master switch over numbering.
pub(crate) fn assign_header_identifiers(blocks: &mut [Block], ext: Extensions, markdown: bool) {
    let mut numbering = HeaderNumbering::new(ext, markdown);
    if numbering.scheme.is_none() {
        return;
    }
    walk(blocks, &mut numbering);
}

/// Hands out header identifiers in reading order, applying the active scheme's disambiguation.
///
/// With no auto-identifier algorithm enabled, a header keeps the id it was written with (an explicit
/// `{#id}`, or empty).
pub(crate) struct HeaderNumbering {
    scheme: Option<IdScheme>,
    /// The broad Markdown dialect, which disambiguates every slug shape with the native strategy.
    markdown: bool,
    registry: IdRegistry,
}

impl HeaderNumbering {
    pub(crate) fn new(ext: Extensions, markdown: bool) -> Self {
        Self {
            scheme: IdScheme::select(ext, markdown),
            markdown,
            registry: IdRegistry::default(),
        }
    }

    /// The id for the next header in reading order: an explicit id is kept (and reserved so later
    /// derived ids avoid it); otherwise one is derived from the header text. Advances disambiguation
    /// state on every call, so an unchanged sequence of calls always yields the same identifiers.
    pub(crate) fn id_for(&mut self, explicit_id: &str, inlines: &[Inline]) -> String {
        let Some(scheme) = self.scheme else {
            return explicit_id.to_owned();
        };
        if explicit_id.is_empty() {
            let text = to_plain_text(inlines);
            if self.markdown {
                self.registry.assign_markdown(scheme, &text)
            } else {
                self.registry.assign(scheme, &text)
            }
        } else {
            if self.markdown {
                self.registry.reserve_native(explicit_id);
            } else {
                self.registry.reserve(scheme, explicit_id);
            }
            explicit_id.to_owned()
        }
    }
}

fn walk(blocks: &mut [Block], numbering: &mut HeaderNumbering) {
    for block in blocks {
        match block {
            Block::Header(_, attr, inlines) => {
                attr.id = numbering.id_for(&attr.id, inlines);
            }
            Block::BlockQuote(children)
            | Block::Div(_, children)
            | Block::Figure(_, _, children) => walk(children, numbering),
            Block::BulletList(items) | Block::OrderedList(_, items) => {
                for item in items {
                    walk(item, numbering);
                }
            }
            Block::DefinitionList(items) => {
                for (_, definitions) in items {
                    for definition in definitions {
                        walk(definition, numbering);
                    }
                }
            }
            Block::Table(table) => walk_table(table, numbering),
            _ => {}
        }
    }
}

fn walk_table(table: &mut Table, numbering: &mut HeaderNumbering) {
    let body_rows = table
        .bodies
        .iter_mut()
        .flat_map(|body| body.head.iter_mut().chain(body.body.iter_mut()));
    let rows = table
        .head
        .rows
        .iter_mut()
        .chain(body_rows)
        .chain(table.foot.rows.iter_mut());
    for row in rows {
        for cell in &mut row.cells {
            walk(&mut cell.content, numbering);
        }
    }
}
