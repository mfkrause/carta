use super::*;

/// Reads with the default option set and reports only whether the read completed without error,
/// so a pathological input can be checked for graceful, bounded-time handling.
fn reads_ok(input: &str) -> bool {
    DokuwikiReader
        .read(input, &ReaderOptions::default())
        .is_ok()
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
