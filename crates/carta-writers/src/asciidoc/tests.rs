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
