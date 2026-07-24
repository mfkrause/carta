//! Bullet, ordered and definition list rendering.

use super::runs::code_paragraph;
use super::tables::render_table;
use super::{
    Ctx, FlowStyle, custom_style, heading_style, paragraph, render_flow, styled_paragraph,
};
use carta_ast::{Block, Inline, ListAttributes};
use carta_core::container::xml::Element;

/// Renders a bulleted list, binding every item to one concrete list number at the given depth.
/// `ambient` is the paragraph style a surrounding custom-style div imposes on the items' loose
/// paragraphs; it is `None` in ordinary flow.
pub(super) fn render_bullet_list(
    items: &[Vec<Block>],
    out: &mut Element,
    ctx: &mut Ctx,
    depth: u32,
    ambient: Option<&str>,
) {
    // A list whose every item leads with a checkbox is a task list: each item binds to its own
    // state-selected checkbox number, with the leading glyph and following space stripped.
    if let Some(states) = task_list_states(items) {
        for (item, checked) in items.iter().zip(states) {
            let num_id = ctx.plan.checkbox(checked);
            let stripped = strip_checkbox(item);
            render_list_item(&stripped, num_id, depth, out, ctx, ambient);
        }
        return;
    }
    let num_id = ctx.plan.bullet();
    for item in items {
        render_list_item(item, num_id, depth, out, ctx, ambient);
    }
}

/// The checked state of every item when the list is a task list (one whose every item leads with a
/// ballot-box marker), else `None`. An empty list is never a task list.
fn task_list_states(items: &[Vec<Block>]) -> Option<Vec<bool>> {
    if items.is_empty() {
        return None;
    }
    items.iter().map(|item| checkbox_state(item)).collect()
}

/// Whether an item leads with a task-list checkbox, and if so whether it is ticked. An item qualifies
/// when its first block is a `Plain` or `Para` whose first inline is the empty or ticked ballot box
/// followed by a space.
fn checkbox_state(item: &[Block]) -> Option<bool> {
    let (Block::Plain(inlines) | Block::Para(inlines)) = item.first()? else {
        return None;
    };
    let checked = match inlines.first()? {
        Inline::Str(text) if text.as_str() == "\u{2610}" => false,
        Inline::Str(text) if text.as_str() == "\u{2612}" => true,
        _ => return None,
    };
    matches!(inlines.get(1), Some(Inline::Space)).then_some(checked)
}

/// A task item's blocks with the leading ballot-box glyph and the space after it removed from the
/// first paragraph, so the checkbox does not double as both the marker and inline text.
fn strip_checkbox(item: &[Block]) -> Vec<Block> {
    let mut blocks = item.to_vec();
    if let Some(Block::Plain(inlines) | Block::Para(inlines)) = blocks.first_mut() {
        let cut = inlines.len().min(2);
        inlines.drain(..cut);
    }
    blocks
}

/// Renders an ordered list, binding every item to the concrete number its marker style, delimiter
/// and start select.
pub(super) fn render_ordered_list(
    attrs: &ListAttributes,
    items: &[Vec<Block>],
    out: &mut Element,
    ctx: &mut Ctx,
    depth: u32,
    ambient: Option<&str>,
) {
    let num_id = ctx.plan.ordered(attrs);
    for item in items {
        render_list_item(item, num_id, depth, out, ctx, ambient);
    }
}

/// Renders one list item: its lead paragraph binds to the list's number, every later block that
/// yields a paragraph binds to the scaffold number so it reads as a continuation line, a nested table
/// is indented to the item's level, and a nested list deepens the level.
fn render_list_item(
    item: &[Block],
    num_id: u32,
    depth: u32,
    out: &mut Element,
    ctx: &mut Ctx,
    ambient: Option<&str>,
) {
    let mut lead_used = false;
    for block in item {
        render_item_block(block, num_id, depth, &mut lead_used, out, ctx, ambient);
    }
}

/// Renders one block of a list item. Each paragraph the block yields binds to the item's own number
/// on the first paragraph and to the continuation number thereafter, keeping the whole item indented
/// under one marker; a nested table takes the item's indent instead; and a nested list that leads
/// the item, with no paragraph ahead of it to carry the marker, is preceded by an empty numbered
/// paragraph that holds the item's marker.
fn render_item_block(
    block: &Block,
    num_id: u32,
    depth: u32,
    lead_used: &mut bool,
    out: &mut Element,
    ctx: &mut Ctx,
    ambient: Option<&str>,
) {
    match block {
        Block::Plain(inlines) => {
            if inlines.is_empty() && !ctx.features.keep_empty_paragraphs {
                return;
            }
            let number = list_number(lead_used, num_id, ctx);
            out.push(styled_paragraph(
                Some("Compact"),
                Some((number, depth)),
                None,
                inlines,
                ctx,
            ));
        }
        // A loose paragraph takes the surrounding custom-style, plus the item's numbering.
        Block::Para(inlines) => {
            if inlines.is_empty() && !ctx.features.keep_empty_paragraphs {
                return;
            }
            let number = list_number(lead_used, num_id, ctx);
            out.push(styled_paragraph(
                ambient,
                Some((number, depth)),
                None,
                inlines,
                ctx,
            ));
        }
        // A nested heading is no outline section: level style, no bookmark, item numbering.
        Block::Header(level, _, inlines) => {
            let style = heading_style(*level);
            let number = list_number(lead_used, num_id, ctx);
            out.push(styled_paragraph(
                Some(style),
                Some((number, depth)),
                None,
                inlines,
                ctx,
            ));
        }
        Block::CodeBlock(attr, code) => {
            let number = list_number(lead_used, num_id, ctx);
            out.push(code_paragraph(
                attr,
                code,
                Some((number, depth)),
                &ctx.highlighter,
            ));
        }
        // Block-quote paragraphs take the quote style and the item's numbering; other blocks
        // render as if directly in the item.
        Block::BlockQuote(blocks) => {
            for inner in blocks {
                match inner {
                    Block::Para(inlines) | Block::Plain(inlines) => {
                        if inlines.is_empty() && !ctx.features.keep_empty_paragraphs {
                            continue;
                        }
                        let number = list_number(lead_used, num_id, ctx);
                        out.push(styled_paragraph(
                            Some("BlockText"),
                            Some((number, depth)),
                            None,
                            inlines,
                            ctx,
                        ));
                    }
                    other => render_item_block(other, num_id, depth, lead_used, out, ctx, ambient),
                }
            }
        }
        Block::Table(table) => render_table(table, out, ctx, Some(depth)),
        Block::BulletList(items) => {
            lead_empty_paragraph(num_id, depth, lead_used, out, ctx);
            render_bullet_list(items, out, ctx, depth + 1, ambient);
        }
        Block::OrderedList(attrs, items) => {
            lead_empty_paragraph(num_id, depth, lead_used, out, ctx);
            render_ordered_list(attrs, items, out, ctx, depth + 1, ambient);
        }
        // A transparent div's paragraphs number like the item's own.
        Block::Div(attr, blocks) if custom_style(attr).is_none() => {
            for inner in blocks {
                render_item_block(inner, num_id, depth, lead_used, out, ctx, ambient);
            }
        }
        other => render_flow(
            other,
            out,
            ctx,
            FlowStyle {
                para: "BodyText",
                plain: "Compact",
                list_ambient: None,
            },
        ),
    }
}

/// Emits the empty paragraph that carries a list item's marker when a nested list leads the item, so
/// the outer item still shows its own number or bullet. Does nothing once the item's lead is spent.
fn lead_empty_paragraph(
    num_id: u32,
    depth: u32,
    lead_used: &mut bool,
    out: &mut Element,
    ctx: &mut Ctx,
) {
    if *lead_used {
        return;
    }
    *lead_used = true;
    out.push(styled_paragraph(
        Some("Compact"),
        Some((num_id, depth)),
        None,
        &[],
        ctx,
    ));
}

/// The number a list item's next paragraph binds to: the item's own on its first paragraph, the
/// scaffold continuation number thereafter.
fn list_number(lead_used: &mut bool, num_id: u32, ctx: &Ctx) -> u32 {
    let number = if *lead_used {
        ctx.plan.continuation_num()
    } else {
        num_id
    };
    *lead_used = true;
    number
}

/// Renders a definition list: each term as a `DefinitionTerm` paragraph and each definition's blocks
/// under the `Definition` style.
pub(super) fn render_definition_list(
    items: &[(Vec<Inline>, Vec<Vec<Block>>)],
    out: &mut Element,
    ctx: &mut Ctx,
) {
    for (term, definitions) in items {
        if !term.is_empty() {
            out.push(paragraph("DefinitionTerm", term, ctx));
        }
        for definition in definitions {
            for block in definition {
                render_definition_block(block, out, ctx);
            }
        }
    }
}

/// Renders one block of a definition's body under the `Definition` style.
fn render_definition_block(block: &Block, out: &mut Element, ctx: &mut Ctx) {
    match block {
        Block::Para(inlines) | Block::Plain(inlines) => {
            if !inlines.is_empty() || ctx.features.keep_empty_paragraphs {
                out.push(paragraph("Definition", inlines, ctx));
            }
        }
        other => render_flow(
            other,
            out,
            ctx,
            FlowStyle {
                para: "Definition",
                plain: "Definition",
                list_ambient: Some("Definition"),
            },
        ),
    }
}
