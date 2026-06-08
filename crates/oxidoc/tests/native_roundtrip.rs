//! Oracle-free round-trip identity tests for the native reader/writer pair, driven entirely through
//! the facade (no pinned binary). Two directions establish mutual consistency:
//!
//! - `read(write(doc)) == doc` over a corpus of documents covering every block, inline, and table
//!   node the pair handles (metadata aside — the writer renders the block list alone).
//! - `write(read(text)) == text` over a corpus of canonical native texts.
//!
//! This is the core justification for co-implementing the pair, and it must pass without the oracle.

// This whole file is test code, where panicking on a known case is the idiomatic assertion.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use oxidoc::ast::{
    Alignment, Attr, Block, Caption, Cell, Citation, CitationMode, ColSpec, ColWidth, Document,
    Format, Inline, ListAttributes, ListNumberDelim, ListNumberStyle, MathType, QuoteType, Row,
    Table, TableBody, TableFoot, TableHead, Target,
};
use oxidoc::{ReaderOptions, WriterOptions};

fn render(document: &Document) -> String {
    oxidoc::writer_for("native")
        .expect("native writer enabled")
        .write(document, &WriterOptions::default())
        .expect("native writer succeeds")
}

fn parse(text: &str) -> Document {
    oxidoc::reader_for("native")
        .expect("native reader enabled")
        .read(text, &ReaderOptions::default())
        .expect("native reader succeeds")
}

fn document(blocks: Vec<Block>) -> Document {
    Document {
        blocks,
        ..Document::default()
    }
}

fn str_inline(text: &str) -> Inline {
    Inline::Str(text.to_owned())
}

fn attr(id: &str, classes: &[&str], attributes: &[(&str, &str)]) -> Attr {
    Attr {
        id: id.to_owned(),
        classes: classes.iter().map(|c| (*c).to_owned()).collect(),
        attributes: attributes
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
    }
}

/// Documents covering every node the native pair handles. Metadata is intentionally empty: the
/// writer renders the block list alone, so it is not part of the write→read identity.
#[allow(clippy::too_many_lines)]
fn document_corpus() -> Vec<Document> {
    let plain = |text: &str| Block::Plain(vec![str_inline(text)]);
    let item = |text: &str| vec![plain(text)];

    vec![
        document(vec![]),
        document(vec![Block::Para(vec![
            str_inline("a"),
            Inline::Space,
            Inline::SoftBreak,
            Inline::LineBreak,
            str_inline("b"),
        ])]),
        document(vec![Block::Para(vec![
            Inline::Emph(vec![str_inline("e")]),
            Inline::Strong(vec![str_inline("s")]),
            Inline::Underline(vec![str_inline("u")]),
            Inline::Strikeout(vec![str_inline("k")]),
            Inline::Superscript(vec![str_inline("2")]),
            Inline::Subscript(vec![str_inline("3")]),
            Inline::SmallCaps(vec![str_inline("c")]),
        ])]),
        document(vec![Block::Para(vec![
            Inline::Quoted(QuoteType::SingleQuote, vec![str_inline("a")]),
            Inline::Quoted(QuoteType::DoubleQuote, vec![str_inline("b")]),
        ])]),
        document(vec![Block::Para(vec![
            Inline::Code(attr("i", &["lang"], &[("k", "v")]), "x = 1".to_owned()),
            Inline::Math(MathType::InlineMath, "a^2".to_owned()),
            Inline::Math(MathType::DisplayMath, "b".to_owned()),
            Inline::RawInline(Format("html".to_owned()), "<b>".to_owned()),
        ])]),
        document(vec![Block::Para(vec![
            Inline::Link(
                attr("l", &["c"], &[]),
                vec![str_inline("t")],
                Target {
                    url: "http://x".to_owned(),
                    title: "ti".to_owned(),
                },
            ),
            Inline::Image(
                Attr::default(),
                vec![str_inline("alt")],
                Target {
                    url: "p.png".to_owned(),
                    title: String::new(),
                },
            ),
            Inline::Span(attr("s", &["c"], &[]), vec![str_inline("inner")]),
            Inline::Note(vec![Block::Para(vec![str_inline("n")])]),
        ])]),
        document(vec![Block::Para(vec![Inline::Cite(
            vec![
                Citation {
                    id: "k".to_owned(),
                    prefix: vec![str_inline("see")],
                    suffix: vec![str_inline("p5")],
                    mode: CitationMode::NormalCitation,
                    note_num: 1,
                    hash: 0,
                },
                Citation {
                    id: "j".to_owned(),
                    prefix: vec![],
                    suffix: vec![],
                    mode: CitationMode::AuthorInText,
                    note_num: 0,
                    hash: 0,
                },
                Citation {
                    id: "h".to_owned(),
                    prefix: vec![],
                    suffix: vec![],
                    mode: CitationMode::SuppressAuthor,
                    note_num: 0,
                    hash: 0,
                },
            ],
            vec![str_inline("[@k]")],
        )])]),
        document(vec![
            Block::LineBlock(vec![vec![str_inline("one")], vec![str_inline("two")]]),
            Block::CodeBlock(attr("", &["rust"], &[]), "let x = 1;".to_owned()),
            Block::RawBlock(Format("html".to_owned()), "<div>".to_owned()),
            Block::BlockQuote(vec![Block::Para(vec![str_inline("q")])]),
            Block::HorizontalRule,
        ]),
        document(vec![
            Block::OrderedList(
                ListAttributes {
                    start: 5,
                    style: ListNumberStyle::Decimal,
                    delim: ListNumberDelim::Period,
                },
                vec![item("a"), item("b")],
            ),
            Block::OrderedList(
                ListAttributes {
                    start: 1,
                    style: ListNumberStyle::LowerRoman,
                    delim: ListNumberDelim::OneParen,
                },
                vec![item("c")],
            ),
            Block::OrderedList(
                ListAttributes {
                    start: 1,
                    style: ListNumberStyle::UpperAlpha,
                    delim: ListNumberDelim::TwoParens,
                },
                vec![item("d")],
            ),
            Block::OrderedList(
                ListAttributes {
                    start: 1,
                    style: ListNumberStyle::Example,
                    delim: ListNumberDelim::DefaultDelim,
                },
                vec![item("e")],
            ),
            Block::OrderedList(
                ListAttributes {
                    start: 1,
                    style: ListNumberStyle::DefaultStyle,
                    delim: ListNumberDelim::Period,
                },
                vec![item("f")],
            ),
            Block::OrderedList(
                ListAttributes {
                    start: 1,
                    style: ListNumberStyle::LowerAlpha,
                    delim: ListNumberDelim::Period,
                },
                vec![item("g")],
            ),
            Block::OrderedList(
                ListAttributes {
                    start: 1,
                    style: ListNumberStyle::UpperRoman,
                    delim: ListNumberDelim::Period,
                },
                vec![item("h")],
            ),
            Block::BulletList(vec![item("a"), item("b")]),
            Block::DefinitionList(vec![(vec![str_inline("Term")], vec![vec![plain("def")]])]),
        ]),
        document(vec![
            Block::Header(2, attr("id", &["c"], &[("k", "v")]), vec![str_inline("H")]),
            Block::Div(
                attr("d", &["note"], &[]),
                vec![Block::Para(vec![str_inline("body")])],
            ),
        ]),
        document(vec![
            Block::Figure(
                attr("f", &[], &[]),
                Caption {
                    short: None,
                    long: vec![Block::Plain(vec![str_inline("cap")])],
                },
                vec![Block::Plain(vec![Inline::Image(
                    Attr::default(),
                    vec![str_inline("alt")],
                    Target {
                        url: "p.png".to_owned(),
                        title: String::new(),
                    },
                )])],
            ),
            Block::Figure(
                Attr::default(),
                Caption {
                    short: Some(vec![str_inline("short")]),
                    long: vec![Block::Plain(vec![str_inline("long")])],
                },
                vec![Block::Para(vec![str_inline("x")])],
            ),
        ]),
        document(vec![Block::Table(Box::new(Table {
            attr: Attr::default(),
            caption: Caption {
                short: None,
                long: vec![Block::Plain(vec![str_inline("cap")])],
            },
            col_specs: vec![
                ColSpec {
                    align: Alignment::AlignLeft,
                    width: ColWidth::ColWidth(0.5),
                },
                ColSpec {
                    align: Alignment::AlignRight,
                    width: ColWidth::ColWidthDefault,
                },
                ColSpec {
                    align: Alignment::AlignCenter,
                    width: ColWidth::ColWidthDefault,
                },
                ColSpec {
                    align: Alignment::AlignDefault,
                    width: ColWidth::ColWidthDefault,
                },
            ],
            head: TableHead {
                attr: Attr::default(),
                rows: vec![Row {
                    attr: Attr::default(),
                    cells: vec![Cell {
                        attr: Attr::default(),
                        align: Alignment::AlignDefault,
                        row_span: 1,
                        col_span: 1,
                        content: vec![Block::Plain(vec![str_inline("h")])],
                    }],
                }],
            },
            bodies: vec![TableBody {
                attr: Attr::default(),
                row_head_columns: 0,
                head: vec![],
                body: vec![Row {
                    attr: Attr::default(),
                    cells: vec![Cell {
                        attr: Attr::default(),
                        align: Alignment::AlignDefault,
                        row_span: 2,
                        col_span: 1,
                        content: vec![Block::Plain(vec![str_inline("a")])],
                    }],
                }],
            }],
            foot: TableFoot {
                attr: Attr::default(),
                rows: vec![],
            },
        }))]),
        // String payloads exercising the writer's escaping and the reader's un-escaping.
        document(vec![Block::Para(vec![
            str_inline("café"),
            str_inline("a\u{1}b"),
            str_inline("tab\there"),
            str_inline("a\"b\\c"),
            str_inline("\u{e9}1"),
        ])]),
    ]
}

/// Canonical native texts: each is exactly what the writer emits, so writing the parse must
/// reproduce it byte-for-byte. Kept short enough to stay on one flat line.
fn text_corpus() -> Vec<&'static str> {
    vec![
        "[]",
        r#"[ Para [ Str "hi" ] ]"#,
        r#"[ Para [ Str "a" , Space , Str "b" ] ]"#,
        r#"[ Para [ Emph [ Str "e" ] , Strong [ Str "s" ] ] ]"#,
        r#"[ Header 1 ( "h" , [] , [] ) [ Str "Hi" ] ]"#,
        "[ HorizontalRule ]",
        r#"[ BulletList [ [ Plain [ Str "a" ] ] ] ]"#,
        r#"[ CodeBlock ( "" , [ "rust" ] , [] ) "x" ]"#,
        r#"[ RawBlock (Format "html") "<b>" ]"#,
        r#"[ Para [ Math InlineMath "a^2" ] ]"#,
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
                "document {index}: round-trip differs\nrendered:\n{text}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn write_after_read_is_identity() {
    let mut failures = Vec::new();
    for text in text_corpus() {
        let rendered = render(&parse(text));
        if rendered != text {
            failures.push(format!("expected {text:?}, got {rendered:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
