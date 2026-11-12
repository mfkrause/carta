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

#[test]
fn citation_suffix_opens_with_a_locator_or_a_comma() {
    for (suffix, expected) in [
        ("p. 5", "[cite:@key p. 5]"),
        (", pp. 5-7, 9", "[cite:@key pp. 5-7, 9]"),
        ("p. 5, see there", "[cite:@key p. 5 , see there]"),
        ("12 and later", "[cite:@key 12 and later]"),
        ("emphasis added", "[cite:@key , emphasis added]"),
        (", emphasis added", "[cite:@key , emphasis added]"),
        ("p.", "[cite:@key , p.]"),
    ] {
        let citation = Citation {
            id: "key".into(),
            prefix: Vec::new(),
            suffix: words(suffix),
            mode: CitationMode::NormalCitation,
            note_num: 0,
            hash: 0,
        };
        let document = Document {
            blocks: vec![Block::Para(vec![Inline::Cite(vec![citation], Vec::new())])],
            ..Document::default()
        };
        let mut options = WriterOptions::default();
        options.extensions.insert(Extension::Citations);
        assert_eq!(OrgWriter.write(&document, &options).unwrap(), expected);
    }
}

#[test]
fn source_block_attributes_become_header_arguments() {
    let attr = Attr {
        id: Text::default(),
        classes: vec!["python".into(), "numberLines".into()],
        attributes: vec![
            ("startFrom".into(), "5".into()),
            ("session".into(), "main".into()),
        ],
    };
    let document = Document {
        blocks: vec![Block::CodeBlock(Box::new(attr), "x".into())],
        ..Document::default()
    };
    assert_eq!(
        OrgWriter
            .write(&document, &WriterOptions::default())
            .unwrap(),
        "#+begin_src python -n 5 :session main\nx\n#+end_src"
    );
}

#[test]
fn tight_list_items_keep_their_own_trailing_space() {
    let document = Document {
        blocks: vec![Block::BulletList(vec![
            vec![Block::Plain(vec![s("a")])],
            vec![
                Block::Plain(vec![s("b")]),
                Block::CodeBlock(Box::default(), "x".into()),
            ],
            vec![Block::Plain(vec![s("c")])],
        ])],
        ..Document::default()
    };
    assert_eq!(
        OrgWriter
            .write(&document, &WriterOptions::default())
            .unwrap(),
        "- a\n- b\n  #+begin_example\n  x\n  #+end_example\n\n- c"
    );
}
