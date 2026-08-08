//! Memo of resolved bracketed citations, shared by every inline parse over one text buffer.
//!
//! Resolving `[ ... @key ... ]` parses its entries' prefixes and suffixes as fresh inline runs,
//! and those runs cover source the enclosing scan already walked, so every enclosing bracket that
//! resolves re-reads everything beneath it. The memo records each bracket's finished resolution
//! the first time it is computed; a later parse over the same span replays the recorded result
//! instead of recomputing it, so nesting citations costs work linear in the recorded results
//! rather than doubling per level.

use std::cell::RefCell;
use std::collections::BTreeMap;

use carta_ast::{Citation, Inline};

/// The bracket content's byte range within the anchored buffer, plus the citation count at the
/// bracket's open. The count is part of the key because the numbering stamped into the result
/// depends on it; an equal count makes the whole resolution byte-identical, so a hit needs no
/// fix-up of any kind.
type Key = (usize, usize, i32);

/// One bracket's finished citation resolution, replayable wherever its [`Key`] recurs.
#[derive(Debug, Clone)]
pub(super) struct ResolvedCitation {
    /// The group's entries, numbering included.
    pub(super) citations: Vec<Citation>,
    /// The literal source rendering carried alongside the entries.
    pub(super) fallback: Vec<Inline>,
    /// The citation count when the resolution finished; a replay restores it directly.
    pub(super) exit_count: i32,
    /// The bracket's length plus the recorded result's node count: what a replay's clone costs,
    /// charged to the citation budget on every replay so repeated replays stay bounded.
    pub(super) cost: usize,
}

/// The memo itself: anchored to one text buffer, keyed by spans within it.
#[derive(Debug)]
pub(super) struct CiteCache {
    /// Address and length of the anchored buffer, held only for span arithmetic.
    base_address: usize,
    base_len: usize,
    entries: RefCell<BTreeMap<Key, ResolvedCitation>>,
}

impl CiteCache {
    pub(super) fn anchored_to(text: &str) -> Self {
        Self {
            base_address: text.as_ptr() as usize,
            base_len: text.len(),
            entries: RefCell::new(BTreeMap::new()),
        }
    }

    /// The byte range `slice` occupies within the anchored buffer, or `None` when it is not part
    /// of it. Live allocations never overlap, so address containment proves the slice is a
    /// subslice of the buffer.
    pub(super) fn range_of(&self, slice: &str) -> Option<(usize, usize)> {
        let start = (slice.as_ptr() as usize).checked_sub(self.base_address)?;
        let end = start.checked_add(slice.len())?;
        (end <= self.base_len).then_some((start, end))
    }

    pub(super) fn get(&self, key: Key) -> Option<ResolvedCitation> {
        self.entries.borrow().get(&key).cloned()
    }

    pub(super) fn insert(&self, key: Key, value: ResolvedCitation) {
        self.entries.borrow_mut().insert(key, value);
    }
}

/// The node count of a recorded resolution: the price of cloning it on replay.
pub(super) fn replay_weight(citations: &[Citation], fallback: &[Inline]) -> usize {
    citations.iter().fold(fallback.len(), |total, citation| {
        total
            .saturating_add(1)
            .saturating_add(inlines_weight(&citation.prefix))
            .saturating_add(inlines_weight(&citation.suffix))
    })
}

fn inlines_weight(inlines: &[Inline]) -> usize {
    inlines.iter().fold(0, |total, inline| {
        let nested = match inline {
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
            | Inline::Span(_, inner) => inlines_weight(inner),
            Inline::Cite(citations, fallback) => replay_weight(citations, fallback),
            Inline::Note(blocks) => blocks_weight(blocks),
            Inline::Str(_)
            | Inline::Code(..)
            | Inline::Space
            | Inline::SoftBreak
            | Inline::LineBreak
            | Inline::Math(..)
            | Inline::RawInline(..) => 0,
        };
        total.saturating_add(1).saturating_add(nested)
    })
}

fn blocks_weight(blocks: &[carta_ast::Block]) -> usize {
    use carta_ast::Block;
    blocks.iter().fold(0, |total, block| {
        let nested = match block {
            Block::Plain(inner) | Block::Para(inner) | Block::Header(_, _, inner) => {
                inlines_weight(inner)
            }
            Block::LineBlock(lines) => lines.iter().map(|line| inlines_weight(line)).sum(),
            Block::BlockQuote(inner) | Block::Div(_, inner) => blocks_weight(inner),
            Block::OrderedList(_, items) | Block::BulletList(items) => {
                items.iter().map(|item| blocks_weight(item)).sum()
            }
            Block::DefinitionList(items) => items
                .iter()
                .map(|(term, definitions)| {
                    inlines_weight(term)
                        + definitions
                            .iter()
                            .map(|blocks| blocks_weight(blocks))
                            .sum::<usize>()
                })
                .sum(),
            Block::Figure(_, caption, inner) => {
                blocks_weight(&caption.long).saturating_add(blocks_weight(inner))
            }
            Block::Table(table) => table_weight(table),
            Block::CodeBlock(..) | Block::RawBlock(..) | Block::HorizontalRule => 0,
        };
        total.saturating_add(1).saturating_add(nested)
    })
}

fn table_weight(table: &carta_ast::Table) -> usize {
    let rows_weight = |rows: &[carta_ast::Row]| {
        rows.iter()
            .flat_map(|row| &row.cells)
            .map(|cell| blocks_weight(&cell.content))
            .sum::<usize>()
    };
    let body_weight: usize = table
        .bodies
        .iter()
        .map(|body| rows_weight(&body.head).saturating_add(rows_weight(&body.body)))
        .sum();
    blocks_weight(&table.caption.long)
        .saturating_add(rows_weight(&table.head.rows))
        .saturating_add(body_weight)
        .saturating_add(rows_weight(&table.foot.rows))
}
