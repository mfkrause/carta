//! Rejoin text separated by soft breaks between wide characters.

use carta_ast::{Block, Document, Inline, MetaValue};

/// Remove soft breaks between wide characters in the document body and parsed metadata.
pub fn drop_document_wide_breaks(document: &mut Document) {
    drop_wide_breaks(&mut document.blocks);
    drop_metadata_wide_breaks(document.meta.values_mut());
}

/// Remove soft breaks between wide characters in parsed metadata values.
pub fn drop_metadata_wide_breaks<'a>(values: impl Iterator<Item = &'a mut MetaValue>) {
    let mut pending: Vec<_> = values.collect();
    while let Some(value) = pending.pop() {
        match value {
            MetaValue::MetaInlines(inlines) => drop_wide_breaks_in(inlines),
            MetaValue::MetaBlocks(blocks) => drop_wide_breaks(blocks),
            MetaValue::MetaList(values) => pending.extend(values),
            MetaValue::MetaMap(values) => pending.extend(values.values_mut()),
            MetaValue::MetaString(_) | MetaValue::MetaBool(_) => {}
        }
    }
}

/// Drops a soft line break falling between two East Asian wide characters, so wrapped text rejoins
/// with no intervening space.
fn drop_wide_breaks(blocks: &mut [Block]) {
    crate::with_stack(|| drop_wide_breaks_nested(blocks));
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
                if let Some(short) = &mut caption.short {
                    drop_wide_breaks_in(short);
                }
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
                if let Some(short) = &mut table.caption.short {
                    drop_wide_breaks_in(short);
                }
                drop_wide_breaks(&mut table.caption.long);
                for row in table
                    .head
                    .rows
                    .iter_mut()
                    .chain(
                        table
                            .bodies
                            .iter_mut()
                            .flat_map(|body| body.head.iter_mut().chain(body.body.iter_mut())),
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
fn drop_wide_breaks_in(inlines: &mut Vec<Inline>) {
    crate::with_stack(|| drop_wide_breaks_in_nested(inlines));
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
    while let Some(inline) = source.next() {
        let joins = matches!(inline, Inline::SoftBreak)
            && source
                .peek()
                .and_then(edge_char::<false>)
                .is_some_and(is_wide)
            && inlines
                .last()
                .and_then(edge_char::<true>)
                .is_some_and(is_wide);
        if !joins {
            inlines.push(inline);
        }
    }
}

/// The character an inline presents at one of its edges: its last when `LAST`, else its first. A
/// quotation presents the mark it is set in rather than its content, and so never a wide one.
fn edge_char<const LAST: bool>(inline: &Inline) -> Option<char> {
    crate::with_stack(|| match inline {
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
    crate::width::is_wide(character as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AnyReader, BytesReader, Extension, Reader, ReaderOptions, Result};
    use carta_ast::{Alignment, Cell, ColSpec, ColWidth, Row, Table, TableBody};

    fn separated() -> Vec<Inline> {
        vec![
            Inline::Str("中".into()),
            Inline::SoftBreak,
            Inline::Str("文".into()),
        ]
    }

    fn joined() -> Vec<Inline> {
        vec![Inline::Str("中".into()), Inline::Str("文".into())]
    }

    #[test]
    fn wide_breaks_keep_narrow_words_hard_breaks_and_literal_content() {
        let unchanged = vec![
            Inline::Str("a".into()),
            Inline::SoftBreak,
            Inline::Str("文".into()),
            Inline::LineBreak,
            Inline::Str("中".into()),
            Inline::Code(Box::default(), "中\n文".into()),
        ];
        let mut document = Document {
            blocks: vec![
                Block::Para(separated()),
                Block::Para(unchanged.clone()),
                Block::CodeBlock(Box::default(), "中\n文".into()),
            ],
            ..Document::default()
        };
        drop_document_wide_breaks(&mut document);
        assert_eq!(
            document.blocks,
            vec![
                Block::Para(joined()),
                Block::Para(unchanged),
                Block::CodeBlock(Box::default(), "中\n文".into())
            ]
        );
    }

    #[test]
    fn wide_breaks_cover_metadata_captions_and_every_table_section() {
        let row = || Row {
            cells: vec![Cell {
                attr: Default::default(),
                align: Alignment::AlignDefault,
                row_span: 1,
                col_span: 1,
                content: vec![Block::Para(separated())],
            }],
            ..Row::default()
        };
        let mut table = Table {
            col_specs: vec![ColSpec {
                align: Alignment::AlignDefault,
                width: ColWidth::ColWidthDefault,
            }],
            bodies: vec![TableBody {
                head: vec![row()],
                body: vec![row()],
                ..TableBody::default()
            }],
            ..Table::default()
        };
        table.caption.short = Some(separated());
        table.caption.long = vec![Block::Para(separated())];
        table.head.rows = vec![row()];
        table.foot.rows = vec![row()];
        let mut document = Document {
            meta: [(
                "title".into(),
                MetaValue::MetaMap(
                    [(
                        "items".into(),
                        MetaValue::MetaList(vec![
                            MetaValue::MetaInlines(separated()),
                            MetaValue::MetaBlocks(vec![Block::Para(separated())]),
                        ]),
                    )]
                    .into(),
                ),
            )]
            .into(),
            blocks: vec![Block::Table(Box::new(table))],
            ..Document::default()
        };
        drop_document_wide_breaks(&mut document);
        let mut remaining = 0;
        crate::walk::for_each_document_inline(&mut document, &mut |inline| {
            remaining += usize::from(matches!(inline, Inline::SoftBreak));
        });
        assert_eq!(remaining, 0);
    }

    struct TextWithBreaks;

    impl Reader for TextWithBreaks {
        fn read(&self, _input: &str, _options: &ReaderOptions) -> Result<Document> {
            Ok(Document {
                blocks: vec![Block::Para(separated())],
                ..Document::default()
            })
        }
    }

    impl BytesReader for TextWithBreaks {
        fn read(&self, _input: &[u8], options: &ReaderOptions) -> Result<Document> {
            Reader::read(self, "", options)
        }
    }

    #[test]
    fn reader_dispatch_applies_the_extension_to_text_and_byte_inputs() {
        for reader in [
            AnyReader::Text(Box::new(TextWithBreaks)),
            AnyReader::Bytes(Box::new(TextWithBreaks)),
        ] {
            let mut options = ReaderOptions::default();
            assert_eq!(
                reader.read(b"", &options).unwrap().blocks,
                vec![Block::Para(separated())]
            );
            assert_eq!(
                reader.read_media(b"", &options).unwrap().0.blocks,
                vec![Block::Para(separated())]
            );
            options.extensions.insert(Extension::EastAsianLineBreaks);
            assert_eq!(
                reader.read(b"", &options).unwrap().blocks,
                vec![Block::Para(joined())]
            );
            assert_eq!(
                reader.read_media(b"", &options).unwrap().0.blocks,
                vec![Block::Para(joined())]
            );
        }
    }
}
