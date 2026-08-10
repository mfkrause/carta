//! Offline round-trip identity for the `DocBook` reader/writer pair, driven through the facade.
//!
//! The corpus holds documents whose shapes survive a full render and reparse unchanged, so
//! `read(write(doc)) == doc` pins the two directions against each other without any external tool.
//! `DocBook` has no vocabulary for several AST distinctions (raw blocks, citations, cell spans, the
//! attributes carried by many wrappers), so those shapes are deliberately absent.

// This whole file is test code, where panicking on a known case is the idiomatic assertion.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg(all(feature = "read-docbook", feature = "write-docbook"))]

use carta::ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Document, Inline, ListAttributes,
    ListNumberDelim, ListNumberStyle, QuoteType, Row, Table, TableBody, TableFoot, TableHead,
    Target,
};
use carta::{ReaderOptions, WriterOptions};

fn render(document: &Document) -> String {
    carta::writer_for("docbook")
        .expect("docbook writer enabled")
        .write(document, &WriterOptions::default())
        .expect("docbook writer succeeds")
}

/// Parses a rendered body, supplying the single root element the writer leaves to the template.
fn parse(body: &str) -> Document {
    let text = format!(
        "<article xmlns=\"http://docbook.org/ns/docbook\" version=\"5.0\">{body}</article>"
    );
    carta::reader_for("docbook")
        .expect("docbook reader enabled")
        .read(&text, &ReaderOptions::default())
        .expect("docbook reader succeeds")
}

fn document(blocks: Vec<Block>) -> Document {
    Document {
        blocks,
        ..Document::default()
    }
}

fn str_inline(text: &str) -> Inline {
    Inline::Str(text.into())
}

fn attr(id: &str, classes: &[&str]) -> Attr {
    Attr {
        id: id.into(),
        classes: classes.iter().map(|c| (*c).into()).collect(),
        attributes: Vec::new(),
    }
}

fn plain(text: &str) -> Block {
    Block::Plain(vec![str_inline(text)])
}

fn cell(text: &str) -> Cell {
    Cell {
        attr: Attr::default(),
        align: Alignment::AlignDefault,
        row_span: 1,
        col_span: 1,
        content: vec![plain(text)],
    }
}

fn row(texts: &[&str]) -> Row {
    Row {
        attr: Attr::default(),
        cells: texts.iter().map(|t| cell(t)).collect(),
    }
}

fn ordered(style: ListNumberStyle, start: i64) -> ListAttributes {
    ListAttributes {
        start,
        style,
        delim: ListNumberDelim::DefaultDelim,
    }
}

#[allow(clippy::too_many_lines)]
fn document_corpus() -> Vec<Document> {
    let item = |text: &str| vec![plain(text)];

    vec![
        document(vec![]),
        document(vec![Block::Para(vec![
            str_inline("a"),
            Inline::Space,
            str_inline("b"),
        ])]),
        document(vec![Block::Para(vec![
            Inline::Emph(vec![str_inline("e")]),
            Inline::Strong(vec![str_inline("s")]),
            Inline::Strikeout(vec![str_inline("k")]),
            Inline::Superscript(vec![str_inline("2")]),
            Inline::Subscript(vec![str_inline("3")]),
        ])]),
        document(vec![Block::Para(vec![Inline::Quoted(
            QuoteType::DoubleQuote,
            vec![str_inline("quoted")],
        )])]),
        document(vec![Block::Para(vec![
            Inline::Code(Box::default(), "x = 1".into()),
            Inline::Note(vec![Block::Para(vec![str_inline("n")])]),
        ])]),
        document(vec![Block::Para(vec![
            Inline::Link(
                Box::default(),
                vec![str_inline("t")],
                Box::new(Target {
                    url: "http://x".into(),
                    title: "".into(),
                }),
            ),
            Inline::Image(
                Box::default(),
                vec![str_inline("alt")],
                Box::new(Target {
                    url: "p.png".into(),
                    title: "".into(),
                }),
            ),
        ])]),
        document(vec![
            Block::CodeBlock(Box::default(), "let x = 1;".into()),
            Block::BlockQuote(vec![Block::Para(vec![str_inline("q")])]),
            Block::LineBlock(vec![vec![str_inline("one")], vec![str_inline("two")]]),
        ]),
        document(vec![
            Block::BulletList(vec![item("a"), item("b")]),
            Block::OrderedList(ordered(ListNumberStyle::Decimal, 5), vec![item("a")]),
            Block::OrderedList(ordered(ListNumberStyle::LowerAlpha, 1), vec![item("b")]),
            Block::OrderedList(ordered(ListNumberStyle::UpperAlpha, 1), vec![item("c")]),
            Block::OrderedList(ordered(ListNumberStyle::LowerRoman, 1), vec![item("d")]),
            Block::OrderedList(ordered(ListNumberStyle::UpperRoman, 1), vec![item("e")]),
            Block::DefinitionList(vec![(
                vec![str_inline("Term")],
                vec![vec![Block::Para(vec![str_inline("def")])]],
            )]),
        ]),
        document(vec![
            Block::Header(1, Box::new(attr("one", &[])), vec![str_inline("One")]),
            Block::Para(vec![str_inline("beneath-one")]),
            Block::Header(2, Box::new(attr("two", &[])), vec![str_inline("Two")]),
            Block::Para(vec![str_inline("beneath-two")]),
        ]),
        document(vec![Block::Div(
            Box::new(attr("", &["warning"])),
            vec![
                Block::Div(
                    Box::new(attr("", &["title"])),
                    vec![Block::Plain(vec![str_inline("Careful")])],
                ),
                Block::Para(vec![str_inline("mind")]),
            ],
        )]),
        document(vec![Block::Table(Box::new(Table {
            attr: Attr::default(),
            caption: Caption {
                short: None,
                long: vec![plain("cap")],
            },
            col_specs: vec![
                ColSpec {
                    align: Alignment::AlignLeft,
                    width: ColWidth::ColWidthDefault,
                },
                ColSpec {
                    align: Alignment::AlignRight,
                    width: ColWidth::ColWidthDefault,
                },
            ],
            head: TableHead {
                attr: Attr::default(),
                rows: vec![row(&["h1", "h2"])],
            },
            bodies: vec![TableBody {
                attr: Attr::default(),
                row_head_columns: 0,
                head: vec![],
                body: vec![row(&["a", "b"]), row(&["c", "d"])],
            }],
            foot: TableFoot {
                attr: Attr::default(),
                rows: vec![],
            },
        }))]),
        // Payloads exercising the writer's escaping and the reader's un-escaping.
        document(vec![
            Block::Para(vec![str_inline("café")]),
            Block::Para(vec![str_inline("a<b>c")]),
            Block::Para(vec![str_inline("a&b")]),
            Block::Para(vec![str_inline("quote\"and'apostrophe")]),
        ]),
    ]
}

#[test]
fn read_after_write_is_identity() {
    let mut failures = Vec::new();
    for (index, doc) in document_corpus().into_iter().enumerate() {
        let text = render(&doc);
        let parsed = parse(&text);
        if parsed != doc {
            failures.push(format!(
                "document {index}: round-trip differs\nrendered:\n{text}\nreparsed:\n{parsed:?}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
