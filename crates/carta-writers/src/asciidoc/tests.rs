use super::*;

fn render(inlines: Vec<Inline>) -> String {
    let document = Document {
        blocks: vec![Block::Para(inlines)],
        ..Document::default()
    };
    AsciidocWriter
        .write(&document, &WriterOptions::default())
        .unwrap()
}

fn s(text: &str) -> Inline {
    Inline::Str(text.to_owned().into())
}

#[test]
fn list_items_keep_placeholders_before_initial_blocks() {
    let document = Document {
        blocks: vec![Block::BulletList(vec![
            vec![Block::CodeBlock(Box::default(), "x".into())],
            vec![Block::OrderedList(
                ListAttributes {
                    start: 1,
                    style: carta_ast::ListNumberStyle::DefaultStyle,
                    delim: carta_ast::ListNumberDelim::DefaultDelim,
                },
                vec![vec![Block::Plain(vec![s("inner")])]],
            )],
            vec![Block::Plain(vec![s("next")])],
        ])],
        ..Document::default()
    };
    assert_eq!(
        AsciidocWriter
            .write(&document, &WriterOptions::default())
            .unwrap(),
        "* {blank}\n+\n....\nx\n....\n* {blank}\n. inner\n* next"
    );
}

#[test]
fn list_items_separate_rules_and_trim_raw_trailing_breaks() {
    let document = Document {
        blocks: vec![Block::BulletList(vec![
            vec![Block::HorizontalRule],
            vec![Block::RawBlock(
                carta_ast::Format("asciidoc".into()),
                "x\n\n\n".into(),
            )],
            vec![Block::BulletList(vec![])],
            vec![Block::Plain(vec![s("next")])],
        ])],
        ..Document::default()
    };
    assert_eq!(
        AsciidocWriter
            .write(&document, &WriterOptions::default())
            .unwrap(),
        "* {blank}\n+\n\n'''''\n* {blank}\n+\nx\n* {blank}\n* next"
    );
}

fn link(label: Vec<Inline>, url: &str) -> Inline {
    Inline::Link(
        Box::default(),
        label,
        Box::new(Target {
            url: url.into(),
            title: String::new().into(),
        }),
    )
}

#[test]
fn decoded_label_link_renders_bare() {
    assert_eq!(
        render(vec![link(
            vec![s("http://e.com/a b")],
            "http://e.com/a%20b"
        )]),
        "http://e.com/a%20b"
    );
}

#[test]
fn exact_label_link_renders_bare() {
    assert_eq!(
        render(vec![link(
            vec![s("http://e.com/a%20b")],
            "http://e.com/a%20b"
        )]),
        "http://e.com/a%20b"
    );
}

#[test]
fn distinct_label_link_renders_explicit() {
    assert_eq!(
        render(vec![link(vec![s("click")], "http://e.com/a%20b")]),
        "http://e.com/a%20b[click]"
    );
}

#[test]
fn legacy_syntax_keeps_code_quotes_math_and_checkbox_glyphs() {
    let document = Document {
        blocks: vec![
            Block::Para(vec![
                Inline::Quoted(carta_ast::QuoteType::DoubleQuote, vec![s("hello")]),
                Inline::Space,
                Inline::Code(Box::default(), "a_`b".into()),
                Inline::Space,
                Inline::Math(MathType::InlineMath, "x".into()),
            ]),
            Block::Para(vec![Inline::Math(MathType::DisplayMath, "x+y".into())]),
            Block::BulletList(vec![vec![Block::Plain(vec![
                s("☐"),
                Inline::Space,
                s("item"),
            ])]]),
        ],
        ..Document::default()
    };
    assert_eq!(
        AsciidocLegacyWriter
            .write(&document, &WriterOptions::default())
            .unwrap(),
        "``hello'' `a_\\'b` latexmath:[$x$]\n\n[latexmath]\n++++\n\\[x+y\\]\n++++\n\n* ☐ item"
    );
}
