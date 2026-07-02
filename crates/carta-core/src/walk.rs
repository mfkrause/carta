//! Mutable traversals over the document model.
//!
//! [`for_each_image_target`] visits every image target in a block sequence, descending through all
//! nested inline and block content in document order. Rewriting a container format's inline resource
//! references — a notebook's `attachment:` links on the way in, its file names on the way out, and an
//! extraction step's on-disk paths — is the same walk with a different callback, so the traversal
//! lives here once rather than in each reader and writer.

use carta_ast::{Block, Caption, Inline, Table, Target};

/// Applies `visit` to every image target throughout `blocks`, descending into every nested inline and
/// block sequence — list items, table cells, notes, captions, and the rest — in document order.
pub fn for_each_image_target(blocks: &mut [Block], visit: &mut dyn FnMut(&mut Target)) {
    for block in blocks {
        visit_block(block, visit);
    }
}

fn visit_block(block: &mut Block, visit: &mut dyn FnMut(&mut Target)) {
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
            for_each_image_target(inner, visit);
        }
        Block::OrderedList(_, items) | Block::BulletList(items) => {
            for item in items {
                for_each_image_target(item, visit);
            }
        }
        Block::DefinitionList(items) => {
            for (term, definitions) in items {
                visit_inlines(term, visit);
                for definition in definitions {
                    for_each_image_target(definition, visit);
                }
            }
        }
        Block::Figure(_, caption, inner) => {
            visit_caption(caption, visit);
            for_each_image_target(inner, visit);
        }
        Block::Table(table) => visit_table(table, visit),
        Block::CodeBlock(..) | Block::RawBlock(..) | Block::HorizontalRule => {}
    }
}

fn visit_table(table: &mut Table, visit: &mut dyn FnMut(&mut Target)) {
    visit_caption(&mut table.caption, visit);
    let row_groups = std::iter::once(&mut table.head.rows)
        .chain(table.bodies.iter_mut().flat_map(|body| {
            std::iter::once(&mut body.head).chain(std::iter::once(&mut body.body))
        }))
        .chain(std::iter::once(&mut table.foot.rows));
    for rows in row_groups {
        for row in rows {
            for cell in &mut row.cells {
                for_each_image_target(&mut cell.content, visit);
            }
        }
    }
}

fn visit_caption(caption: &mut Caption, visit: &mut dyn FnMut(&mut Target)) {
    if let Some(short) = &mut caption.short {
        visit_inlines(short, visit);
    }
    for_each_image_target(&mut caption.long, visit);
}

fn visit_inlines(inlines: &mut [Inline], visit: &mut dyn FnMut(&mut Target)) {
    for inline in inlines {
        match inline {
            Inline::Image(_, alt, target) => {
                visit(target);
                visit_inlines(alt, visit);
            }
            Inline::Link(_, children, _)
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
            Inline::Note(blocks) => for_each_image_target(blocks, visit),
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
    use super::for_each_image_target;
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

    #[test]
    fn visits_images_nested_in_containers() {
        let mut blocks = vec![
            Block::Para(vec![image("a")]),
            Block::BulletList(vec![vec![Block::Plain(vec![image("b")])]]),
            Block::BlockQuote(vec![Block::Div(
                Box::new(Attr::default()),
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
        // The mutation is threaded back into the tree.
        let Block::Para(inlines) = &blocks[0] else {
            panic!("expected para");
        };
        let Inline::Image(_, _, target) = &inlines[0] else {
            panic!("expected image");
        };
        assert_eq!(target.url.as_str(), "seen:a");
    }
}
