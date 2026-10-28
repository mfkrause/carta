//! Rejoin text separated by soft breaks between wide characters.

use carta_ast::{Block, Inline};

/// Drops a soft line break falling between two East Asian wide characters, so wrapped text rejoins
/// with no intervening space.
pub(crate) fn drop_wide_breaks(blocks: &mut [Block]) {
    carta_core::with_stack(|| drop_wide_breaks_nested(blocks));
}

fn drop_wide_breaks_nested(blocks: &mut [Block]) {
    for block in blocks.iter_mut() {
        match block {
            Block::Plain(inlines) | Block::Para(inlines) | Block::Header(_, _, inlines) => {
                drop_wide_breaks_in(inlines);
            }
            Block::LineBlock(lines) => lines.iter_mut().for_each(drop_wide_breaks_in),
            Block::BlockQuote(children) | Block::Div(_, children) => drop_wide_breaks(children),
            Block::Figure(_, caption, children) => {
                drop_wide_breaks(&mut caption.long);
                drop_wide_breaks(children);
            }
            Block::OrderedList(_, items) | Block::BulletList(items) => {
                for item in items {
                    drop_wide_breaks(item);
                }
            }
            Block::DefinitionList(entries) => {
                for (term, definitions) in entries {
                    drop_wide_breaks_in(term);
                    for definition in definitions {
                        drop_wide_breaks(definition);
                    }
                }
            }
            Block::Table(table) => {
                drop_wide_breaks(&mut table.caption.long);
                for row in table
                    .head
                    .rows
                    .iter_mut()
                    .chain(
                        table
                            .bodies
                            .iter_mut()
                            .flat_map(|body| body.body.iter_mut()),
                    )
                    .chain(table.foot.rows.iter_mut())
                {
                    for cell in &mut row.cells {
                        drop_wide_breaks(&mut cell.content);
                    }
                }
            }
            Block::CodeBlock(_, _) | Block::RawBlock(_, _) | Block::HorizontalRule => {}
        }
    }
}

/// Rejoins wide text within an inline sequence and its nested content.
pub(crate) fn drop_wide_breaks_in(inlines: &mut Vec<Inline>) {
    carta_core::with_stack(|| drop_wide_breaks_in_nested(inlines));
}

fn drop_wide_breaks_in_nested(inlines: &mut Vec<Inline>) {
    for inline in inlines.iter_mut() {
        match inline {
            Inline::Emph(children)
            | Inline::Underline(children)
            | Inline::Strong(children)
            | Inline::Strikeout(children)
            | Inline::Superscript(children)
            | Inline::Subscript(children)
            | Inline::SmallCaps(children)
            | Inline::Quoted(_, children)
            | Inline::Cite(_, children)
            | Inline::Link(_, children, _)
            | Inline::Image(_, children, _)
            | Inline::Span(_, children) => drop_wide_breaks_in(children),
            Inline::Note(children) => drop_wide_breaks(children),
            _ => {}
        }
    }
    let original = std::mem::take(inlines);
    inlines.reserve(original.len());
    let mut source = original.into_iter().peekable();
    let mut before_wide = false;
    while let Some(inline) = source.next() {
        let joins = matches!(inline, Inline::SoftBreak)
            && before_wide
            && source
                .peek()
                .and_then(edge_char::<false>)
                .is_some_and(is_wide);
        if !joins {
            before_wide = matches!(source.peek(), Some(Inline::SoftBreak))
                && edge_char::<true>(&inline).is_some_and(is_wide);
            inlines.push(inline);
        }
    }
}

/// The character an inline presents at one of its edges: its last when `LAST`, else its first. A
/// quotation presents the mark it is set in rather than its content, and so never a wide one.
fn edge_char<const LAST: bool>(inline: &Inline) -> Option<char> {
    carta_core::with_stack(|| match inline {
        Inline::Str(text) | Inline::Code(_, text) | Inline::Math(_, text) => {
            if LAST {
                text.chars().next_back()
            } else {
                text.chars().next()
            }
        }
        Inline::Emph(children)
        | Inline::Underline(children)
        | Inline::Strong(children)
        | Inline::Strikeout(children)
        | Inline::Superscript(children)
        | Inline::Subscript(children)
        | Inline::SmallCaps(children)
        | Inline::Cite(_, children)
        | Inline::Link(_, children, _)
        | Inline::Image(_, children, _)
        | Inline::Span(_, children) => {
            if LAST {
                children.iter().rev().find_map(edge_char::<LAST>)
            } else {
                children.iter().find_map(edge_char::<LAST>)
            }
        }
        _ => None,
    })
}

fn is_wide(character: char) -> bool {
    carta_core::width::is_wide(character as u32)
}
