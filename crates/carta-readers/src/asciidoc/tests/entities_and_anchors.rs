use super::super::AsciidocReader;
use carta_ast::{Attr, Block, Inline};
use carta_core::{Reader, ReaderOptions};

fn read(source: &str, tab_stop: usize) -> carta_ast::Document {
    let mut options = ReaderOptions::default();
    options.tab_stop = tab_stop;
    AsciidocReader.read(source, &options).unwrap()
}

fn paragraph(source: &str, tab_stop: usize) -> Vec<Inline> {
    let document = read(source, tab_stop);
    match document.blocks.as_slice() {
        [Block::Para(inlines)] => inlines.clone(),
        blocks => panic!("expected one paragraph, got {blocks:?}"),
    }
}

#[test]
fn attribute_lists_follow_anchor_priority() {
    assert_eq!(
        read("[[]\n=", 4).blocks,
        vec![Block::Para(vec![Inline::Str("=".into())])]
    );
    for source in ["[[]\nText\n", "[[id]\nText\n"] {
        assert_eq!(
            read(source, 4).blocks,
            vec![Block::Para(vec![Inline::Str("Text".into())])],
            "{source:?}"
        );
    }

    assert_eq!(
        read("[[[id]]\nText\n", 4).blocks,
        vec![Block::Div(
            Box::new(Attr {
                id: "[id".into(),
                attributes: vec![("wrapper".into(), "1".into())],
                ..Attr::default()
            }),
            vec![Block::Para(vec![Inline::Str("Text".into())])]
        )]
    );

    let document = read("[[id]] \nText\n", 4);
    let [Block::Para(inlines)] = document.blocks.as_slice() else {
        panic!("an anchor with trailing space must stay inline");
    };
    assert!(matches!(
        inlines.first(),
        Some(Inline::Span(attr, _)) if attr.id == "id"
    ));
}

#[test]
fn character_references_use_the_source_terminator_rules() {
    for (source, tab_stop, expected) in [
        ("&amp\tw", 0, "&w"),
        ("&amp!", 4, "&amp!"),
        ("&amp", 4, "&amp"),
        ("&#65X", 4, "&#65X"),
        ("&#X41;X", 4, "AX"),
        ("&#x41X", 4, "&#x41X"),
        ("&#xZZ;", 4, "&#xZZ;"),
        ("&#;", 4, "&#;"),
        ("&#18446744073709551616;X", 4, "\0X"),
        ("&#x10000000000000000;X", 4, "\0X"),
    ] {
        assert_eq!(
            paragraph(source, tab_stop),
            [Inline::Str(expected.into())],
            "{source:?}"
        );
    }
    assert_eq!(
        paragraph("&amp  w", 4),
        [
            Inline::Str("&".into()),
            Inline::Space,
            Inline::Str("w".into())
        ]
    );
}

#[test]
fn entity_and_attribute_whitespace_matches_the_asciidoc_source() {
    for (separator, expected) in [
        ('\u{00a0}', "&X".to_owned()),
        ('\u{0085}', "&amp\u{0085}X".to_owned()),
        ('\u{2028}', "&amp\u{2028}X".to_owned()),
        ('\u{2029}', "&amp\u{2029}X".to_owned()),
    ] {
        assert_eq!(
            paragraph(&format!("&amp{separator}X"), 4),
            vec![Inline::Str(expected.into())],
            "{separator:?}"
        );
    }

    for (separator, expected) in [
        ('\u{00a0}', "red".to_owned()),
        ('\u{0085}', "red\u{0085}".to_owned()),
        ('\u{2028}', "red\u{2028}".to_owned()),
        ('\u{2029}', "red\u{2029}".to_owned()),
    ] {
        let document = read(&format!("[role=red{separator}]\nText\n"), 4);
        let [Block::Div(attr, _)] = document.blocks.as_slice() else {
            panic!("expected a block attribute wrapper");
        };
        assert_eq!(attr.classes, vec![expected], "{separator:?}");
    }
}
