use super::*;

fn attr(pairs: &[(&str, &str)]) -> Attr {
    Attr {
        attributes: pairs
            .iter()
            .map(|(key, value)| ((*key).into(), (*value).into()))
            .collect(),
        ..Attr::default()
    }
}

#[test]
fn image_size_converts_dimensions_to_pixels() {
    assert_eq!(
        image_size(&attr(&[("width", "1in"), ("height", "0.5in")])),
        Some("96x48px".to_owned())
    );
    assert_eq!(
        image_size(&attr(&[("width", "2in")])),
        Some("192px".to_owned())
    );
    assert_eq!(
        image_size(&attr(&[("height", "1in")])),
        Some("x96px".to_owned())
    );
    assert_eq!(
        image_size(&attr(&[("width", "120px")])),
        Some("120px".to_owned())
    );
    assert_eq!(image_size(&attr(&[("width", "50%")])), None);
    assert_eq!(image_size(&attr(&[])), None);
}

#[test]
fn plain_runs_on_into_the_next_block() {
    let plain = Block::Plain(Vec::new());
    let para = Block::Para(Vec::new());
    assert_eq!(separator(&plain, &para, false), "\n");
    assert_eq!(separator(&plain, &Block::HorizontalRule, false), "\n\n");
    assert_eq!(separator(&para, &plain, false), "\n\n");
}

#[test]
fn definition_lists_keep_html_context_through_containers() {
    let list = Block::BulletList(vec![
        vec![Block::Plain(vec![Inline::Str("alpha".into())])],
        vec![Block::Plain(vec![Inline::Str("beta".into())])],
    ]);
    for (content, expected) in [
        (
            list.clone(),
            "<ol start=\"2\">\n<li><dl>\n<dt>term</dt>\n<dd><ul>\n<li>alpha</li>\n<li>beta</li></ul></dd></dl></li></ol>",
        ),
        (
            Block::Div(Box::default(), vec![list]),
            "<ol start=\"2\">\n<li><dl>\n<dt>term</dt>\n<dd><div>\n<ul>\n<li>alpha</li>\n<li>beta</li></ul>\n\n</div></dd></dl></li></ol>",
        ),
    ] {
        let document = Document {
            blocks: vec![Block::OrderedList(
                ListAttributes {
                    start: 2,
                    style: ListNumberStyle::DefaultStyle,
                    delim: carta_ast::ListNumberDelim::DefaultDelim,
                },
                vec![vec![Block::DefinitionList(vec![(
                    vec![Inline::Str("term".into())],
                    vec![vec![content]],
                )])]],
            )],
            ..Document::default()
        };
        assert_eq!(
            MediawikiWriter
                .write(&document, &WriterOptions::default())
                .unwrap(),
            expected
        );
    }
}
