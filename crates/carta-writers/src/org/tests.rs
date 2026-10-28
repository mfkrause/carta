use super::*;

fn render(inlines: Vec<Inline>) -> String {
    let document = Document {
        blocks: vec![Block::Para(inlines)],
        ..Document::default()
    };
    OrgWriter
        .write(&document, &WriterOptions::default())
        .unwrap()
}

fn s(text: &str) -> Inline {
    Inline::Str(text.to_owned().into())
}

#[test]
fn list_markers_stand_above_initial_blocks() {
    for (block, expected) in [
        (
            Block::CodeBlock(Box::default(), "x".into()),
            "- \n  #+begin_example\n  x\n  #+end_example",
        ),
        (
            Block::BlockQuote(vec![Block::Para(vec![s("quoted")])]),
            "- \n\n  #+begin_quote\n  quoted\n  #+end_quote",
        ),
    ] {
        let document = Document {
            blocks: vec![Block::BulletList(vec![vec![block]])],
            ..Document::default()
        };
        assert_eq!(
            OrgWriter
                .write(&document, &WriterOptions::default())
                .unwrap(),
            expected
        );
    }
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
fn decoded_label_link_renders_single_segment() {
    assert_eq!(
        render(vec![link(
            vec![s("http://e.com/a b")],
            "http://e.com/a%20b"
        )]),
        "[[http://e.com/a b]]"
    );
}

#[test]
fn exact_label_link_renders_single_segment() {
    assert_eq!(
        render(vec![link(
            vec![s("http://e.com/a%20b")],
            "http://e.com/a%20b"
        )]),
        "[[http://e.com/a%20b]]"
    );
}

#[test]
fn distinct_label_link_renders_two_segments() {
    assert_eq!(
        render(vec![link(vec![s("click")], "http://e.com/a%20b")]),
        "[[http://e.com/a%20b][click]]"
    );
}

#[test]
fn oversized_cell_spans_stay_bounded() {
    use carta_ast::{
        Alignment, Attr, Caption, Cell, ColSpec, ColWidth, Table, TableFoot, TableHead,
    };
    let cell = Cell {
        attr: Attr::default(),
        align: Alignment::AlignLeft,
        row_span: i64::MAX,
        col_span: i64::MAX,
        content: Vec::new(),
    };
    let table = Table {
        attr: Attr::default(),
        caption: Caption::default(),
        col_specs: vec![ColSpec {
            align: Alignment::AlignLeft,
            width: ColWidth::ColWidthDefault,
        }],
        head: TableHead {
            attr: Attr::default(),
            rows: vec![Row {
                attr: Attr::default(),
                cells: vec![cell],
            }],
        },
        bodies: Vec::new(),
        foot: TableFoot::default(),
    };
    // Spans dwarfing the grid must clamp to real slots, not iterate the full span product.
    let document = Document {
        blocks: vec![Block::Table(Box::new(table))],
        ..Document::default()
    };
    let output = OrgWriter
        .write(&document, &WriterOptions::default())
        .unwrap();
    assert!(
        output.len() < 1_000,
        "unbounded table output: {} bytes",
        output.len()
    );
}

fn words(text: &str) -> Vec<Inline> {
    let mut inlines = Vec::new();
    for word in text.split_whitespace() {
        if !inlines.is_empty() {
            inlines.push(Inline::Space);
        }
        inlines.push(s(word));
    }
    inlines
}

#[test]
fn styled_inlines_wrap_between_words() {
    let text = "alpha beta gamma delta";
    for (inline, expected) in [
        (Inline::Emph(words(text)), "/alpha beta\ngamma delta/"),
        (Inline::Strong(words(text)), "*alpha beta\ngamma delta*"),
        (Inline::Underline(words(text)), "_alpha beta\ngamma delta_"),
        (Inline::Strikeout(words(text)), "+alpha beta\ngamma delta+"),
        (
            Inline::Superscript(words(text)),
            "^{alpha beta\ngamma delta}",
        ),
        (Inline::Subscript(words(text)), "_{alpha beta\ngamma delta}"),
    ] {
        let document = Document {
            blocks: vec![Block::Para(vec![inline])],
            ..Document::default()
        };
        let mut options = WriterOptions::default();
        options.columns = Some(12);
        assert_eq!(OrgWriter.write(&document, &options).unwrap(), expected);
    }
}

#[test]
fn definition_terms_and_nested_bullets_share_the_first_line() {
    for (term, expected) in [
        (
            "term",
            "- term :: - alpha\n    beta gamma delta\n    epsilon zeta eta\n    theta iota",
        ),
        (
            "alpha beta gamma",
            "- alpha beta\n  gamma :: - alpha\n    beta gamma delta\n    epsilon zeta eta\n    theta iota",
        ),
    ] {
        let document = Document {
            blocks: vec![Block::DefinitionList(vec![(
                words(term),
                vec![vec![Block::BulletList(vec![vec![Block::Plain(words(
                    "alpha beta gamma delta epsilon zeta eta theta iota",
                ))]])]],
            )])],
            ..Document::default()
        };
        let mut options = WriterOptions::default();
        options.columns = Some(20);
        assert_eq!(OrgWriter.write(&document, &options).unwrap(), expected);
    }
}
