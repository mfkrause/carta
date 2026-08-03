//! A ceiling on how far a writer may amplify the document it renders.
//!
//! Some formats emit already-rendered content more than once: a `longtable` head repeats so it can
//! be reprinted after a page break, a grid table pads every cell in a column out to the widest one.
//! Nesting such a construct inside itself multiplies the output at every level, so a few kilobytes
//! of input can render to gigabytes.
//!
//! [`scope`] installs a ceiling proportional to the document's own size for the duration of one
//! write. A writer whose output can grow faster than its input calls [`charge`] with what it has
//! rendered and stops early once [`exhausted`] reports the ceiling is passed; the write then fails
//! with [`Error::OutputTooLarge`] instead of exhausting memory.
//!
//! A construct that multiplies its own content must consult [`exhausted`] *after* rendering that
//! content as well as before: nested constructs charge as the recursion unwinds, so checking only
//! on the way in still lets every enclosing level multiply what it already holds.

use std::cell::Cell;

use carta_ast::{Attr, Block, Caption, Document, Inline, MetaValue, Table, Text};

use crate::{Error, Result};

/// Output allowed before the document contributes anything, covering the fixed chrome of a
/// standalone document (preamble, stylesheet, script).
const BASE: usize = 1 << 20;

/// Output allowed per unit of document weight. Generous enough that no faithful rendering reaches
/// it: the widest per-node chrome across the formats is a few hundred bytes.
const PER_WEIGHT: usize = 1024;

#[derive(Clone, Copy)]
struct State {
    limit: usize,
    spent: usize,
}

thread_local! {
    static STATE: Cell<Option<State>> = const { Cell::new(None) };
}

/// Restores the enclosing state even if the work between unwinds.
struct Active(Option<State>);

impl Drop for Active {
    fn drop(&mut self) {
        STATE.set(self.0);
    }
}

/// Runs `work` under a ceiling derived from `document`, failing with [`Error::OutputTooLarge`] when
/// the writers charging against it render past that ceiling.
///
/// Reentrant: a writer that delegates to another format's writer shares the outermost ceiling
/// rather than granting itself a fresh one.
///
/// # Errors
/// Propagates any error from `work`, or [`Error::OutputTooLarge`] when the ceiling is passed.
pub fn scope<T>(document: &Document, work: impl FnOnce() -> Result<T>) -> Result<T> {
    if STATE.get().is_some() {
        return work();
    }
    let restore = Active(None);
    STATE.set(Some(State {
        limit: limit_for(document),
        spent: 0,
    }));
    let rendered = work();
    let passed = exhausted();
    drop(restore);
    if passed {
        return Err(Error::OutputTooLarge);
    }
    rendered
}

/// Records `bytes` of rendered output against the active ceiling. Does nothing outside a [`scope`].
pub fn charge(bytes: usize) {
    if let Some(state) = STATE.get() {
        STATE.set(Some(State {
            spent: state.spent.saturating_add(bytes),
            ..state
        }));
    }
}

/// Whether rendering has passed the active ceiling, so a caller should stop producing output.
/// Always `false` outside a [`scope`].
#[must_use]
pub fn exhausted() -> bool {
    STATE.get().is_some_and(|state| state.spent > state.limit)
}

/// Runs `work` and then refunds whatever it charged, for a rendering that is measured and thrown
/// away rather than emitted. The ceiling still applies while `work` runs, so a throwaway rendering
/// is bounded like any other; it just does not count against the output that is kept.
pub fn refunded<T>(work: impl FnOnce() -> T) -> T {
    let before = STATE.get();
    let measured = work();
    STATE.set(before);
    measured
}

fn limit_for(document: &Document) -> usize {
    BASE.saturating_add(weight(document).saturating_mul(PER_WEIGHT))
}

/// What the document is worth as output: one unit per node plus the length of every text it
/// carries, so a document dominated by one long code block is weighed by that text rather than by
/// its single node.
fn weight(document: &Document) -> usize {
    let mut total = 0usize;
    let mut pending: Vec<Node<'_>> = Vec::new();
    pending.extend(document.blocks.iter().map(Node::Block));
    pending.extend(
        document
            .meta
            .iter()
            .flat_map(|(key, value)| [Node::Text(key.len()), Node::Meta(value)]),
    );

    while let Some(node) = pending.pop() {
        total = total.saturating_add(1);
        match node {
            Node::Text(len) => total = total.saturating_add(len),
            Node::Block(block) => push_block(block, &mut pending),
            Node::Inline(inline) => push_inline(inline, &mut pending),
            Node::Meta(meta) => push_meta(meta, &mut pending),
        }
    }
    total
}

enum Node<'a> {
    Block(&'a Block),
    Inline(&'a Inline),
    Meta(&'a MetaValue),
    Text(usize),
}

fn push_block<'a>(block: &'a Block, pending: &mut Vec<Node<'a>>) {
    match block {
        Block::Plain(inlines) | Block::Para(inlines) => {
            pending.extend(inlines.iter().map(Node::Inline));
        }
        Block::LineBlock(lines) => {
            pending.extend(lines.iter().flatten().map(Node::Inline));
        }
        Block::CodeBlock(attr, text) => {
            push_attr(attr, pending);
            pending.push(Node::Text(text.len()));
        }
        Block::RawBlock(format, text) => {
            pending.push(Node::Text(format.0.len().saturating_add(text.len())));
        }
        Block::BlockQuote(blocks) => pending.extend(blocks.iter().map(Node::Block)),
        Block::OrderedList(_, items) | Block::BulletList(items) => {
            pending.extend(items.iter().flatten().map(Node::Block));
        }
        Block::DefinitionList(entries) => {
            for (term, definitions) in entries {
                pending.extend(term.iter().map(Node::Inline));
                pending.extend(definitions.iter().flatten().map(Node::Block));
            }
        }
        Block::Header(_, attr, inlines) => {
            push_attr(attr, pending);
            pending.extend(inlines.iter().map(Node::Inline));
        }
        Block::HorizontalRule => {}
        Block::Table(table) => push_table(table, pending),
        Block::Figure(attr, caption, blocks) => {
            push_attr(attr, pending);
            push_caption(caption, pending);
            pending.extend(blocks.iter().map(Node::Block));
        }
        Block::Div(attr, blocks) => {
            push_attr(attr, pending);
            pending.extend(blocks.iter().map(Node::Block));
        }
    }
}

fn push_inline<'a>(inline: &'a Inline, pending: &mut Vec<Node<'a>>) {
    match inline {
        Inline::Str(text) | Inline::Math(_, text) => pending.push(Node::Text(text.len())),
        Inline::Emph(inlines)
        | Inline::Underline(inlines)
        | Inline::Strong(inlines)
        | Inline::Strikeout(inlines)
        | Inline::Superscript(inlines)
        | Inline::Subscript(inlines)
        | Inline::SmallCaps(inlines)
        | Inline::Quoted(_, inlines) => pending.extend(inlines.iter().map(Node::Inline)),
        Inline::Cite(citations, inlines) => {
            for citation in citations {
                pending.push(Node::Text(citation.id.len()));
                pending.extend(citation.prefix.iter().map(Node::Inline));
                pending.extend(citation.suffix.iter().map(Node::Inline));
            }
            pending.extend(inlines.iter().map(Node::Inline));
        }
        Inline::Code(attr, text) => {
            push_attr(attr, pending);
            pending.push(Node::Text(text.len()));
        }
        Inline::Space | Inline::SoftBreak | Inline::LineBreak => {}
        Inline::RawInline(format, text) => {
            pending.push(Node::Text(format.0.len().saturating_add(text.len())));
        }
        Inline::Link(attr, inlines, target) | Inline::Image(attr, inlines, target) => {
            push_attr(attr, pending);
            pending.extend(inlines.iter().map(Node::Inline));
            pending.push(Node::Text(
                target.url.len().saturating_add(target.title.len()),
            ));
        }
        Inline::Note(blocks) => pending.extend(blocks.iter().map(Node::Block)),
        Inline::Span(attr, inlines) => {
            push_attr(attr, pending);
            pending.extend(inlines.iter().map(Node::Inline));
        }
    }
}

fn push_meta<'a>(meta: &'a MetaValue, pending: &mut Vec<Node<'a>>) {
    match meta {
        MetaValue::MetaMap(map) => pending.extend(
            map.iter()
                .flat_map(|(key, value)| [Node::Text(key.len()), Node::Meta(value)]),
        ),
        MetaValue::MetaList(values) => pending.extend(values.iter().map(Node::Meta)),
        MetaValue::MetaBool(_) => {}
        MetaValue::MetaString(text) => pending.push(Node::Text(text.len())),
        MetaValue::MetaInlines(inlines) => pending.extend(inlines.iter().map(Node::Inline)),
        MetaValue::MetaBlocks(blocks) => pending.extend(blocks.iter().map(Node::Block)),
    }
}

fn push_table<'a>(table: &'a Table, pending: &mut Vec<Node<'a>>) {
    push_attr(&table.attr, pending);
    push_caption(&table.caption, pending);
    let rows = table
        .head
        .rows
        .iter()
        .chain(
            table
                .bodies
                .iter()
                .flat_map(|body| body.head.iter().chain(body.body.iter())),
        )
        .chain(table.foot.rows.iter());
    for row in rows {
        for cell in &row.cells {
            push_attr(&cell.attr, pending);
            pending.extend(cell.content.iter().map(Node::Block));
        }
    }
}

fn push_caption<'a>(caption: &'a Caption, pending: &mut Vec<Node<'a>>) {
    if let Some(short) = &caption.short {
        pending.extend(short.iter().map(Node::Inline));
    }
    pending.extend(caption.long.iter().map(Node::Block));
}

fn push_attr<'a>(attr: &'a Attr, pending: &mut Vec<Node<'a>>) {
    let text = attr
        .id
        .len()
        .saturating_add(attr.classes.iter().map(Text::len).sum())
        .saturating_add(
            attr.attributes
                .iter()
                .map(|(key, value)| key.len().saturating_add(value.len()))
                .sum(),
        );
    pending.push(Node::Text(text));
}

#[cfg(test)]
mod tests {
    use carta_ast::{Alignment, Block, Cell, ColSpec, ColWidth, Document, Inline, Row, Text};

    use super::{charge, exhausted, scope};
    use crate::Error;

    fn document(blocks: Vec<Block>) -> Document {
        Document {
            blocks,
            ..Document::default()
        }
    }

    #[test]
    fn charging_within_the_ceiling_returns_the_rendered_value() {
        let document = document(vec![Block::Plain(vec![Inline::Str(Text::from("x"))])]);
        let rendered = scope(&document, || {
            charge(1024);
            assert!(!exhausted());
            Ok("output")
        });
        assert_eq!(rendered.ok(), Some("output"));
    }

    #[test]
    fn charging_past_the_ceiling_fails_the_write() {
        let document = document(vec![Block::Plain(vec![Inline::Str(Text::from("x"))])]);
        let rendered: Result<(), Error> = scope(&document, || {
            charge(usize::MAX);
            assert!(exhausted());
            Ok(())
        });
        assert!(matches!(rendered, Err(Error::OutputTooLarge)));
    }

    #[test]
    fn a_larger_document_earns_a_larger_ceiling() {
        let small = document(vec![Block::Plain(vec![Inline::Str(Text::from("x"))])]);
        let large = document(vec![Block::Plain(
            (0..10_000).map(|_| Inline::Str(Text::from("x"))).collect(),
        )]);
        let spend = 4 << 20;
        assert!(
            scope(&small, || {
                charge(spend);
                Ok(())
            })
            .is_err()
        );
        assert!(
            scope(&large, || {
                charge(spend);
                Ok(())
            })
            .is_ok()
        );
    }

    #[test]
    fn a_nested_scope_shares_the_outer_ceiling() {
        let document = document(vec![Block::Plain(vec![Inline::Str(Text::from("x"))])]);
        let rendered: Result<(), Error> = scope(&document, || {
            charge(usize::MAX);
            // An inner writer sees the exhausted outer ceiling and must not clear it.
            scope(&document, || Ok(()))?;
            assert!(exhausted());
            Ok(())
        });
        assert!(matches!(rendered, Err(Error::OutputTooLarge)));
    }

    #[test]
    fn the_ceiling_is_lifted_once_the_write_ends() {
        let document = document(vec![Block::Plain(vec![Inline::Str(Text::from("x"))])]);
        let _: Result<(), Error> = scope(&document, || {
            charge(usize::MAX);
            Ok(())
        });
        assert!(!exhausted());
    }

    #[test]
    fn table_cell_content_counts_toward_the_ceiling() {
        let cell = Cell {
            attr: carta_ast::Attr::default(),
            align: Alignment::AlignDefault,
            row_span: 1,
            col_span: 1,
            content: (0..10_000)
                .map(|_| Block::Plain(vec![Inline::Str(Text::from("x"))]))
                .collect(),
        };
        let table = Block::Table(Box::new(carta_ast::Table {
            col_specs: vec![ColSpec {
                align: Alignment::AlignDefault,
                width: ColWidth::ColWidthDefault,
            }],
            head: carta_ast::TableHead {
                rows: vec![Row {
                    cells: vec![cell],
                    ..Row::default()
                }],
                ..carta_ast::TableHead::default()
            },
            ..carta_ast::Table::default()
        }));
        assert!(
            scope(&document(vec![table]), || {
                charge(4 << 20);
                Ok(())
            })
            .is_ok()
        );
    }
}
