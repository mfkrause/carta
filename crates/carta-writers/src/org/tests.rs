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
