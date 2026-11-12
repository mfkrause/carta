//! Bullet, ordered and definition list rendering.

use super::{Ctx, Inherited, Item, render_blocks, styled_paragraph};
use crate::common::{checkbox_state, strip_checkbox};
use carta_ast::{Block, Inline, ListAttributes};
use carta_core::container::xml::Element;

/// Renders a bulleted list, binding every item to one concrete list number one level below the
/// enclosing list item.
pub(super) fn render_bullet_list(
    items: &[Vec<Block>],
    out: &mut Element,
    ctx: &mut Ctx,
    inherited: Inherited,
) {
    // All items must start with checkboxes; each gets state-specific numbering.
    if let Some(states) = task_list_states(items) {
        for (item, checked) in items.iter().zip(states) {
            let num_id = ctx.plan.checkbox(checked);
            let stripped = strip_checkbox(item);
            render_list_item(&stripped, num_id, out, ctx, inherited);
        }
        return;
    }
    let num_id = ctx.plan.bullet();
    for item in items {
        render_list_item(item, num_id, out, ctx, inherited);
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

/// Renders an ordered list, binding every item to the concrete number its marker style, delimiter
/// and start select.
pub(super) fn render_ordered_list(
    attrs: &ListAttributes,
    items: &[Vec<Block>],
    out: &mut Element,
    ctx: &mut Ctx,
    inherited: Inherited,
) {
    let num_id = ctx.plan.ordered(attrs);
    for item in items {
        render_list_item(item, num_id, out, ctx, inherited);
    }
}

/// Renders one list item. The first paragraph its blocks yield binds to the list's number and every
/// later one to the continuation number, so the whole item reads indented under one marker. A
/// nested list that opens the item is preceded by an empty paragraph that carries the marker.
fn render_list_item(
    item: &[Block],
    num_id: u32,
    out: &mut Element,
    ctx: &mut Ctx,
    inherited: Inherited,
) {
    let depth = ctx.item.map_or(0, |outer| outer.depth + 1);
    let outer = ctx.item.replace(Item {
        num_id,
        depth,
        lead_used: false,
    });
    let outer_in_list = std::mem::replace(&mut ctx.in_list, true);
    if matches!(
        item.first(),
        Some(Block::BulletList(_) | Block::OrderedList(..))
    ) {
        let numbering = ctx.take_number();
        out.push(styled_paragraph(
            Some("Compact"),
            numbering,
            inherited.jc,
            &[],
            ctx,
        ));
    }
    render_blocks(item, out, ctx, inherited);
    // A nested list uses up the marker of the item that holds it.
    ctx.item = outer.map(|item| Item {
        lead_used: true,
        ..item
    });
    ctx.in_list = outer_in_list;
}

/// Renders a definition list: each term as a `DefinitionTerm` paragraph and each definition's blocks
/// under the `Definition` style.
pub(super) fn render_definition_list(
    items: &[(Vec<Inline>, Vec<Vec<Block>>)],
    out: &mut Element,
    ctx: &mut Ctx,
    inherited: Inherited,
) {
    for (term, definitions) in items {
        if !term.is_empty() {
            let numbering = ctx.take_number();
            out.push(styled_paragraph(
                Some("DefinitionTerm"),
                numbering,
                inherited.jc,
                term,
                ctx,
            ));
        }
        for definition in definitions {
            render_blocks(definition, out, ctx, inherited.styled("Definition"));
        }
    }
}
