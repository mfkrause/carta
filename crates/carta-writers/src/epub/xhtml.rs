//! Preserve rich content within the XHTML 1.1 content model.

use carta_ast::{Alignment, Attr, Block, Cell, Document, Inline, Row, TableBody};
use carta_core::walk;

pub(super) fn prepare_inlines(document: &mut Document) {
    walk::for_each_document_inline(document, &mut |inline| {
        if let Inline::Underline(children) = inline {
            *inline = Inline::Span(
                Box::new(Attr {
                    attributes: vec![("style".into(), "text-decoration: underline".into())],
                    ..Attr::default()
                }),
                std::mem::take(children),
            );
        }
        if let Inline::Span(attr, _) = inline {
            let mut style = String::new();
            attr.classes.retain(|class| match class.as_str() {
                "mark" => {
                    style.push_str("background-color: yellow; ");
                    false
                }
                "underline" => {
                    style.push_str("text-decoration: underline; ");
                    false
                }
                _ => true,
            });
            if style.is_empty() {
                return;
            }
            if let Some((_, existing)) =
                attr.attributes.iter_mut().find(|(name, _)| name == "style")
            {
                style.push_str(existing);
                *existing = style.into();
            } else {
                attr.attributes
                    .push(("style".into(), style.trim_end().into()));
            }
        }
    });
}

pub(super) fn prepare_tables(blocks: &mut [Block]) {
    walk::for_each_block(blocks, &mut |block| {
        let Block::Table(table) = block else {
            return;
        };
        // XHTML 1.1 tables need at least one row.
        if table.head.rows.is_empty()
            && table.foot.rows.is_empty()
            && table
                .bodies
                .iter()
                .all(|body| body.head.is_empty() && body.body.is_empty())
        {
            let row = Row {
                cells: (0..table.col_specs.len().max(1))
                    .map(|_| Cell {
                        attr: Attr::default(),
                        align: Alignment::AlignDefault,
                        row_span: 1,
                        col_span: 1,
                        content: Vec::new(),
                    })
                    .collect(),
                ..Row::default()
            };
            if let Some(body) = table.bodies.first_mut() {
                body.body.push(row);
            } else {
                table.bodies.push(TableBody {
                    body: vec![row],
                    ..TableBody::default()
                });
            }
        }
        // XHTML 1.1 captions only allow inline content.
        if table
            .caption
            .long
            .iter()
            .any(|block| !matches!(block, Block::Plain(_)))
        {
            let caption = std::mem::take(&mut table.caption.long);
            let table = std::mem::take(table);
            *block = Block::Div(
                Box::default(),
                vec![
                    Block::Div(
                        Box::new(Attr {
                            classes: vec!["table-caption".into()],
                            attributes: vec![(
                                "style".into(),
                                "font-style: italic; margin-bottom: 0.4em; text-align: center"
                                    .into(),
                            )],
                            ..Attr::default()
                        }),
                        caption,
                    ),
                    Block::Table(table),
                ],
            );
        }
    });
}
