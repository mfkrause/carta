//! Mutable document-order traversals for blocks, image targets, and link targets.

use carta_ast::{Attr, Block, Caption, Document, Inline, MetaValue, Row, Table, Target};

/// Apply `visit` to all block, inline, table, row, and cell attributes, including metadata.
pub fn for_each_document_attribute(document: &mut Document, visit: &mut dyn FnMut(&mut Attr)) {
    for_each_document_block(document, &mut |block| match block {
        Block::Header(_, attr, _)
        | Block::CodeBlock(attr, _)
        | Block::Div(attr, _)
        | Block::Figure(attr, _, _) => visit(attr),
        Block::Table(table) => {
            visit(&mut table.attr);
            visit(&mut table.head.attr);
            for body in &mut table.bodies {
                visit(&mut body.attr);
            }
            visit(&mut table.foot.attr);
            for rows in row_groups(table) {
                for row in rows {
                    visit(&mut row.attr);
                    for cell in &mut row.cells {
                        visit(&mut cell.attr);
                    }
                }
            }
        }
        _ => {}
    });
    for_each_document_inline(document, &mut |inline| match inline {
        Inline::Code(attr, _)
        | Inline::Span(attr, _)
        | Inline::Link(attr, _, _)
        | Inline::Image(attr, _, _) => visit(attr),
        _ => {}
    });
}

/// Apply `visit` to every block in the document body and metadata.
pub fn for_each_document_block(document: &mut Document, visit: &mut dyn FnMut(&mut Block)) {
    for_each_block(&mut document.blocks, visit);
    for value in document.meta.values_mut() {
        metadata_blocks(value, visit);
    }
}

fn metadata_blocks(value: &mut MetaValue, visit: &mut dyn FnMut(&mut Block)) {
    crate::with_stack(|| match value {
        MetaValue::MetaMap(values) => {
            for value in values.values_mut() {
                metadata_blocks(value, visit);
            }
        }
        MetaValue::MetaList(values) => {
            for value in values {
                metadata_blocks(value, visit);
            }
        }
        MetaValue::MetaBlocks(blocks) => for_each_block(blocks, visit),
        MetaValue::MetaInlines(inlines) => blocks_in_inlines(inlines, visit),
        MetaValue::MetaBool(_) | MetaValue::MetaString(_) => {}
    });
}

/// Applies `visit` to every image target throughout `blocks`, descending into every nested inline and
/// block sequence (list items, table cells, notes, captions, and the rest) in document order.
pub fn for_each_image_target(blocks: &mut [Block], visit: &mut dyn FnMut(&mut Target)) {
    block_inlines(blocks, &mut |inline| {
        if let Inline::Image(_, _, target) = inline {
            visit(target);
        }
    });
}

/// Applies `visit` to images in the document body and all metadata values.
pub fn for_each_document_image_target(document: &mut Document, visit: &mut dyn FnMut(&mut Target)) {
    for_each_document_inline(document, &mut |inline| {
        if let Inline::Image(_, _, target) = inline {
            visit(target);
        }
    });
}

/// Applies `visit` to each inline in the body and metadata, before descending into its children.
pub fn for_each_document_inline(document: &mut Document, visit: &mut dyn FnMut(&mut Inline)) {
    block_inlines(&mut document.blocks, visit);
    for value in document.meta.values_mut() {
        metadata_inlines(value, visit);
    }
}

fn metadata_inlines(value: &mut MetaValue, visit: &mut dyn FnMut(&mut Inline)) {
    crate::with_stack(|| metadata_inlines_nested(value, visit));
}

fn metadata_inlines_nested(value: &mut MetaValue, visit: &mut dyn FnMut(&mut Inline)) {
    match value {
        MetaValue::MetaMap(values) => {
            for value in values.values_mut() {
                metadata_inlines(value, visit);
            }
        }
        MetaValue::MetaList(values) => {
            for value in values {
                metadata_inlines(value, visit);
            }
        }
        MetaValue::MetaBlocks(blocks) => block_inlines(blocks, visit),
        MetaValue::MetaInlines(inlines) => visit_inlines(inlines, visit),
        MetaValue::MetaBool(_) | MetaValue::MetaString(_) => {}
    }
}

/// Applies `visit` to every link target throughout `blocks`, descending into every nested inline and
/// block sequence in document order, the same traversal as [`for_each_image_target`].
pub fn for_each_link_target(blocks: &mut [Block], visit: &mut dyn FnMut(&mut Target)) {
    block_inlines(blocks, &mut |inline| {
        if let Inline::Link(_, _, target) = inline {
            visit(target);
        }
    });
}

/// Applies `visit` to every block throughout `blocks`, descending into every nested block sequence
/// (list items, table cells, notes, captions, and the rest) in document order. A block is reported
/// before the blocks it holds, so a callback may replace one and still have the replacement walked.
pub fn for_each_block(blocks: &mut [Block], visit: &mut dyn FnMut(&mut Block)) {
    crate::with_stack(|| for_each_block_nested(blocks, visit));
}

fn for_each_block_nested(blocks: &mut [Block], visit: &mut dyn FnMut(&mut Block)) {
    for block in blocks {
        visit(block);
        descend_blocks(block, visit);
    }
}

fn descend_blocks(block: &mut Block, visit: &mut dyn FnMut(&mut Block)) {
    match block {
        Block::BlockQuote(inner) | Block::Div(_, inner) => for_each_block(inner, visit),
        Block::OrderedList(_, items) | Block::BulletList(items) => {
            for item in items {
                for_each_block(item, visit);
            }
        }
        Block::DefinitionList(items) => {
            for (term, definitions) in items {
                blocks_in_inlines(term, visit);
                for definition in definitions {
                    for_each_block(definition, visit);
                }
            }
        }
        Block::Figure(_, caption, inner) => {
            if let Some(short) = &mut caption.short {
                blocks_in_inlines(short, visit);
            }
            for_each_block(&mut caption.long, visit);
            for_each_block(inner, visit);
        }
        Block::Table(table) => {
            if let Some(short) = &mut table.caption.short {
                blocks_in_inlines(short, visit);
            }
            for_each_block(&mut table.caption.long, visit);
            for rows in row_groups(table) {
                for row in rows {
                    for cell in &mut row.cells {
                        for_each_block(&mut cell.content, visit);
                    }
                }
            }
        }
        Block::Plain(inlines) | Block::Para(inlines) | Block::Header(_, _, inlines) => {
            blocks_in_inlines(inlines, visit);
        }
        Block::LineBlock(lines) => {
            for line in lines {
                blocks_in_inlines(line, visit);
            }
        }
        Block::CodeBlock(..) | Block::RawBlock(..) | Block::HorizontalRule => {}
    }
}

/// The blocks an inline run carries, which only a footnote holds.
fn blocks_in_inlines(inlines: &mut [Inline], visit: &mut dyn FnMut(&mut Block)) {
    crate::with_stack(|| blocks_in_inlines_nested(inlines, visit));
}

fn blocks_in_inlines_nested(inlines: &mut [Inline], visit: &mut dyn FnMut(&mut Block)) {
    for inline in inlines {
        match inline {
            Inline::Note(blocks) => for_each_block(blocks, visit),
            Inline::Emph(children)
            | Inline::Underline(children)
            | Inline::Strong(children)
            | Inline::Strikeout(children)
            | Inline::Superscript(children)
            | Inline::Subscript(children)
            | Inline::SmallCaps(children)
            | Inline::Quoted(_, children)
            | Inline::Span(_, children)
            | Inline::Link(_, children, _)
            | Inline::Image(_, children, _) => blocks_in_inlines(children, visit),
            Inline::Cite(citations, children) => {
                for citation in citations {
                    blocks_in_inlines(&mut citation.prefix, visit);
                    blocks_in_inlines(&mut citation.suffix, visit);
                }
                blocks_in_inlines(children, visit);
            }
            Inline::Str(_)
            | Inline::Code(..)
            | Inline::Space
            | Inline::SoftBreak
            | Inline::LineBreak
            | Inline::Math(..)
            | Inline::RawInline(..) => {}
        }
    }
}

/// Every row sequence a table holds, head through foot.
fn row_groups(table: &mut Table) -> impl Iterator<Item = &mut Vec<Row>> {
    std::iter::once(&mut table.head.rows)
        .chain(table.bodies.iter_mut().flat_map(|body| {
            std::iter::once(&mut body.head).chain(std::iter::once(&mut body.body))
        }))
        .chain(std::iter::once(&mut table.foot.rows))
}

fn block_inlines(blocks: &mut [Block], visit: &mut dyn FnMut(&mut Inline)) {
    for block in blocks {
        visit_block(block, visit);
    }
}

fn visit_block(block: &mut Block, visit: &mut dyn FnMut(&mut Inline)) {
    crate::with_stack(|| visit_block_nested(block, visit));
}

fn visit_block_nested(block: &mut Block, visit: &mut dyn FnMut(&mut Inline)) {
    match block {
        Block::Plain(inlines) | Block::Para(inlines) | Block::Header(_, _, inlines) => {
            visit_inlines(inlines, visit);
        }
        Block::LineBlock(lines) => {
            for line in lines {
                visit_inlines(line, visit);
            }
        }
        Block::BlockQuote(inner) | Block::Div(_, inner) => {
            block_inlines(inner, visit);
        }
        Block::OrderedList(_, items) | Block::BulletList(items) => {
            for item in items {
                block_inlines(item, visit);
            }
        }
        Block::DefinitionList(items) => {
            for (term, definitions) in items {
                visit_inlines(term, visit);
                for definition in definitions {
                    block_inlines(definition, visit);
                }
            }
        }
        Block::Figure(_, caption, inner) => {
            visit_caption(caption, visit);
            block_inlines(inner, visit);
        }
        Block::Table(table) => visit_table(table, visit),
        Block::CodeBlock(..) | Block::RawBlock(..) | Block::HorizontalRule => {}
    }
}

fn visit_table(table: &mut Table, visit: &mut dyn FnMut(&mut Inline)) {
    visit_caption(&mut table.caption, visit);
    let row_groups = std::iter::once(&mut table.head.rows)
        .chain(table.bodies.iter_mut().flat_map(|body| {
            std::iter::once(&mut body.head).chain(std::iter::once(&mut body.body))
        }))
        .chain(std::iter::once(&mut table.foot.rows));
    for rows in row_groups {
        for row in rows {
            for cell in &mut row.cells {
                block_inlines(&mut cell.content, visit);
            }
        }
    }
}

fn visit_caption(caption: &mut Caption, visit: &mut dyn FnMut(&mut Inline)) {
    if let Some(short) = &mut caption.short {
        visit_inlines(short, visit);
    }
    block_inlines(&mut caption.long, visit);
}

fn visit_inlines(inlines: &mut [Inline], visit: &mut dyn FnMut(&mut Inline)) {
    crate::with_stack(|| visit_inlines_nested(inlines, visit));
}

fn visit_inlines_nested(inlines: &mut [Inline], visit: &mut dyn FnMut(&mut Inline)) {
    for inline in inlines {
        visit(inline);
        match inline {
            Inline::Image(_, children, _)
            | Inline::Link(_, children, _)
            | Inline::Emph(children)
            | Inline::Underline(children)
            | Inline::Strong(children)
            | Inline::Strikeout(children)
            | Inline::Superscript(children)
            | Inline::Subscript(children)
            | Inline::SmallCaps(children)
            | Inline::Quoted(_, children)
            | Inline::Span(_, children) => visit_inlines(children, visit),
            Inline::Cite(citations, children) => {
                for citation in citations {
                    visit_inlines(&mut citation.prefix, visit);
                    visit_inlines(&mut citation.suffix, visit);
                }
                visit_inlines(children, visit);
            }
            Inline::Note(blocks) => block_inlines(blocks, visit),
            Inline::Str(_)
            | Inline::Code(..)
            | Inline::Space
            | Inline::SoftBreak
            | Inline::LineBreak
            | Inline::Math(..)
            | Inline::RawInline(..) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{for_each_image_target, for_each_link_target};
    use carta_ast::{Attr, Block, Inline, Target};

    fn image(url: &str) -> Inline {
        Inline::Image(
            Box::default(),
            Vec::new(),
            Box::new(Target {
                url: url.into(),
                title: carta_ast::Text::default(),
            }),
        )
    }

    fn link(url: &str, text: &str) -> Inline {
        Inline::Link(
            Box::default(),
            vec![Inline::Str(text.into())],
            Box::new(Target {
                url: url.into(),
                title: carta_ast::Text::default(),
            }),
        )
    }

    #[test]
    fn visits_images_nested_in_containers() {
        let mut blocks = vec![
            Block::Para(vec![image("a")]),
            Block::BulletList(vec![vec![Block::Plain(vec![image("b")])]]),
            Block::BlockQuote(vec![Block::Div(
                Box::<Attr>::default(),
                vec![Block::Para(vec![Inline::Note(vec![Block::Para(vec![
                    image("c"),
                ])])])],
            )]),
        ];
        let mut seen = Vec::new();
        for_each_image_target(&mut blocks, &mut |target| {
            seen.push(target.url.to_string());
            target.url = format!("seen:{}", target.url).into();
        });
        assert_eq!(seen, ["a", "b", "c"]);
        let Some(Block::Para(inlines)) = blocks.first() else {
            panic!("expected para");
        };
        let Some(Inline::Image(_, _, target)) = inlines.first() else {
            panic!("expected image");
        };
        assert_eq!(target.url.as_str(), "seen:a");
    }

    #[test]
    fn visits_links_but_not_images_and_vice_versa() {
        let mut blocks = vec![Block::Para(vec![
            link("l", "go"),
            image("i"),
            Inline::Note(vec![Block::Para(vec![link("n", "note-link")])]),
        ])];
        let mut links = Vec::new();
        for_each_link_target(&mut blocks, &mut |target| {
            links.push(target.url.to_string());
        });
        assert_eq!(links, ["l", "n"]);
        let mut images = Vec::new();
        for_each_image_target(&mut blocks, &mut |target| {
            images.push(target.url.to_string());
        });
        assert_eq!(images, ["i"]);
    }
    #[test]
    fn document_inline_walk_reaches_nested_metadata_and_replacements() {
        use carta_ast::{Document, MetaValue};
        let mut document = Document::default();
        document.blocks.push(Block::Para(vec![image("body")]));
        document.meta.insert(
            "abstract".into(),
            MetaValue::MetaMap(
                [(
                    "items".into(),
                    MetaValue::MetaList(vec![
                        MetaValue::MetaInlines(vec![image("inline")]),
                        MetaValue::MetaBlocks(vec![Block::Para(vec![image("block")])]),
                    ]),
                )]
                .into(),
            ),
        );
        let mut urls = Vec::new();
        super::for_each_document_inline(&mut document, &mut |inline| {
            if let Inline::Image(_, _, target) = inline {
                urls.push(target.url.to_string());
                *inline = Inline::Span(Box::default(), vec![Inline::Str("replacement".into())]);
            }
        });
        assert_eq!(urls, ["body", "inline", "block"]);
        let mut count = 0;
        super::for_each_document_inline(&mut document, &mut |inline| {
            if matches!(inline, Inline::Str(text) if text == "replacement") {
                count += 1;
            }
        });
        assert_eq!(count, 3);
    }

    #[test]
    fn document_attributes_include_table_parts_caption_notes_and_metadata() {
        use carta_ast::{
            Alignment, Caption, Cell, Document, MetaValue, Row, Table, TableBody, TableFoot,
            TableHead,
        };
        let named = |id: &str| Attr {
            id: id.into(),
            ..Attr::default()
        };
        let note =
            |id: &str| Inline::Note(vec![Block::CodeBlock(Box::new(named(id)), "code".into())]);
        let table = Table {
            attr: named("table"),
            caption: Caption {
                short: Some(vec![note("table-note")]),
                ..Caption::default()
            },
            head: TableHead {
                attr: named("header"),
                rows: vec![Row {
                    attr: named("row"),
                    cells: vec![Cell {
                        attr: named("cell"),
                        align: Alignment::AlignDefault,
                        row_span: 1,
                        col_span: 1,
                        content: vec![Block::Header(
                            1,
                            Box::new(named("cell-content")),
                            Vec::new(),
                        )],
                    }],
                }],
            },
            bodies: vec![TableBody {
                attr: named("body"),
                ..TableBody::default()
            }],
            foot: TableFoot {
                attr: named("footer"),
                ..TableFoot::default()
            },
            ..Table::default()
        };
        let mut document = Document {
            blocks: vec![Block::Figure(
                Box::new(named("figure")),
                Box::new(Caption {
                    short: Some(vec![note("figure-note")]),
                    ..Caption::default()
                }),
                vec![Block::Table(Box::new(table))],
            )],
            ..Document::default()
        };
        document.meta.insert(
            "nested".into(),
            MetaValue::MetaList(vec![MetaValue::MetaInlines(vec![Inline::Span(
                Box::new(named("meta-inline")),
                vec![note("meta-block")],
            )])]),
        );
        let mut seen = Vec::new();
        super::for_each_document_attribute(&mut document, &mut |attr| {
            if !attr.id.is_empty() {
                seen.push(attr.id.to_string());
            }
        });
        seen.sort();
        assert_eq!(
            seen,
            [
                "body",
                "cell",
                "cell-content",
                "figure",
                "figure-note",
                "footer",
                "header",
                "meta-block",
                "meta-inline",
                "row",
                "table",
                "table-note"
            ]
        );
    }
}
