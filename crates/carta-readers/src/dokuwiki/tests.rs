use super::*;

/// Reads with the default option set and reports only whether the read completed without error,
/// so a pathological input can be checked for graceful, bounded-time handling.
fn reads_ok(input: &str) -> bool {
    DokuwikiReader
        .read(input, &ReaderOptions::default())
        .is_ok()
}

#[test]
fn tab_stops_apply_to_code_regions() {
    use carta_ast::Block;
    for (width, text) in [(0, "a\tb"), (3, "a  b"), (4, "a b"), (8, "a b")] {
        let mut options = ReaderOptions::default();
        options.tab_stop = width;
        assert_eq!(
            DokuwikiReader
                .read("<code>a\tb</code>", &options)
                .unwrap()
                .blocks,
            vec![Block::CodeBlock(Box::default(), text.into())]
        );
    }
}

#[test]
fn tab_width_changes_list_nesting() {
    use carta_ast::Block;
    let mut options = ReaderOptions::default();
    options.tab_stop = 3;
    let source = "  - one\n\t- two\n";
    let document = DokuwikiReader.read(source, &options).unwrap();
    let Some(Block::OrderedList(_, items)) = document.blocks.first() else {
        panic!("expected list");
    };
    assert_eq!(items.len(), 2);
    options.tab_stop = 4;
    let document = DokuwikiReader.read(source, &options).unwrap();
    let Some(Block::OrderedList(_, items)) = document.blocks.first() else {
        panic!("expected list");
    };
    assert_eq!(items.len(), 1);
    assert!(matches!(
        items.first().and_then(|blocks| blocks.last()),
        Some(Block::OrderedList(_, _))
    ));
}

#[test]
fn adversarial_footnotes_under_open_emphasis_do_not_stall() {
    let input = format!("(({}))", "//((x)) ".repeat(400));
    assert!(reads_ok(&input));
}

#[test]
fn adversarially_nested_footnotes_do_not_stall() {
    let input = format!("{}x{}", "((".repeat(2_000), "))".repeat(2_000));
    assert!(reads_ok(&input));
}

#[test]
fn nested_footnotes_with_invalid_closing_spaces_stay_literal() {
    use carta_ast::{Block, Inline};

    for space in [' ', '\t'] {
        let input = format!("{}x{space}))", "(".repeat(2_000));
        let document = DokuwikiReader
            .read(&input, &ReaderOptions::default())
            .unwrap();
        assert_eq!(
            document.blocks,
            vec![Block::Para(vec![
                Inline::Str(format!("{}x", "(".repeat(2_000)).into()),
                Inline::Space,
                Inline::Str("))".into()),
            ])]
        );
    }
}

#[test]
fn a_delimiter_dense_run_does_not_blow_up() {
    // Repeated unclosed delimiters must not cause quadratic scans.
    let input = "//a ".repeat(4_000);
    assert!(reads_ok(&input));
}

#[test]
fn footnotes_keep_nested_notes_and_protected_closers() {
    use carta_ast::{Block, Inline, to_plain_text};

    let document = DokuwikiReader
        .read("((outer ((inner)) end))", &ReaderOptions::default())
        .unwrap();
    let Block::Para(outer) = &document.blocks[0] else {
        panic!()
    };
    let Inline::Note(blocks) = &outer[0] else {
        panic!()
    };
    let Block::Para(inner) = &blocks[0] else {
        panic!()
    };
    assert!(matches!(&inner[2], Inline::Note(_)));
    assert_eq!(to_plain_text(inner), "outer  end");

    for body in ["%%a))b%%", "[[page|a))b]]", "''a))b''", "**a))b**"] {
        let document = DokuwikiReader
            .read(&format!("((note {body} end))"), &ReaderOptions::default())
            .unwrap();
        let Block::Para(inlines) = &document.blocks[0] else {
            panic!()
        };
        assert_eq!(inlines.len(), 1);
        let Inline::Note(blocks) = &inlines[0] else {
            panic!()
        };
        let Block::Para(inner) = &blocks[0] else {
            panic!()
        };
        assert_eq!(to_plain_text(inner), "note a))b end");
    }
}

#[test]
fn footnote_content_stays_inline_and_rejects_edge_spaces() {
    use carta_ast::{Block, Inline};

    let document = DokuwikiReader
        .read("((> text))", &ReaderOptions::default())
        .unwrap();
    let Block::Para(inlines) = &document.blocks[0] else {
        panic!()
    };
    let Inline::Note(blocks) = &inlines[0] else {
        panic!()
    };
    assert!(matches!(&blocks[0], Block::Para(_)));
    for input in ["(( note))", "((note ))", "((a )) end))"] {
        let document = DokuwikiReader
            .read(input, &ReaderOptions::default())
            .unwrap();
        let Block::Para(inlines) = &document.blocks[0] else {
            panic!()
        };
        assert!(
            !inlines
                .iter()
                .any(|inline| matches!(inline, Inline::Note(_)))
        );
    }
}

#[test]
fn registered_schemes_link_without_an_authority_prefix() {
    use carta_ast::{Block, Inline};

    for address in [
        "mailto:user@example.com",
        "tel:+123456",
        "news:comp.lang.rust",
        "urn:isbn:123",
        "https:example.com",
        "file:/tmp/document",
        "https://example.com",
    ] {
        let document = DokuwikiReader
            .read(&format!("{address}."), &ReaderOptions::default())
            .unwrap();
        let Block::Para(inlines) = &document.blocks[0] else {
            panic!()
        };
        let Inline::Link(_, label, target) = &inlines[0] else {
            panic!()
        };
        assert_eq!(target.url.as_str(), address);
        assert_eq!(label, &vec![Inline::Str(address.into())]);
        assert_eq!(inlines[1], Inline::Str(".".into()));
    }
    for text in [
        "www.example.com",
        "user@example.com",
        "unknown:address",
        "mailto:",
    ] {
        let document = DokuwikiReader
            .read(text, &ReaderOptions::default())
            .unwrap();
        assert_eq!(
            document.blocks,
            vec![Block::Para(vec![Inline::Str(text.into())])]
        );
    }
}

#[test]
fn uri_labels_keep_text_and_escape_destination_delimiters() {
    use carta_ast::Inline;

    let context = Ctx {
        smart: false,
        math: false,
    };
    for (input, label, destination) in [
        ("mailto:[name]", "mailto:[name]", "mailto:%5Bname%5D"),
        ("mailto:a&amp;b", "mailto:a&b", "mailto:a&b"),
        ("mailto:a&#32;b", "mailto:a b", "mailto:a%20b"),
        ("mailto:a&nbsp;b", "mailto:a\u{a0}b", "mailto:a%C2%A0b"),
        ("doi:10.1/example", "doi:10.1/example", "doi:10.1/example"),
    ] {
        let inlines = inline::inline_content(input, context);
        let Inline::Link(_, text, target) = &inlines[0] else {
            panic!()
        };
        assert_eq!(text, &vec![Inline::Str(label.into())]);
        assert_eq!(target.url.as_str(), destination);
    }
    for input in [
        "mailto:a(one)",
        "mailto:_name",
        "mailto:a&copy;",
        "mailto:a_",
    ] {
        let inlines = inline::inline_content(input, context);
        assert!(
            !inlines
                .iter()
                .any(|inline| matches!(inline, Inline::Link(..)))
        );
    }
    let inlines = inline::inline_content("mailto:a\\\\ b", context);
    assert_eq!(
        inlines,
        vec![
            Inline::Str("mailto:a".into()),
            Inline::LineBreak,
            Inline::Str("b".into())
        ]
    );
}
