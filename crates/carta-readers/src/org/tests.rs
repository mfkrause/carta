// Test code: a wrong index panics the test rather than corrupting shipped output.
#![allow(clippy::indexing_slicing)]
use super::*;
use carta_ast::{Inline, QuoteType};

fn doc(input: &str) -> Document {
    let mut options = ReaderOptions::default();
    options.extensions = Extensions::from_list(&[
        Extension::AutoIdentifiers,
        Extension::Citations,
        Extension::TaskLists,
    ]);
    OrgReader.read(input, &options).unwrap()
}

fn blocks(input: &str) -> Vec<Block> {
    doc(input).blocks
}

#[test]
fn paragraph_with_emphasis() {
    let b = blocks("Hello *world* /italic/ =verb= ~code~ +strike+.");
    assert_eq!(b.len(), 1);
    match &b[0] {
        Block::Para(inlines) => {
            assert!(inlines.contains(&Inline::Strong(vec![Inline::Str("world".into())])));
            assert!(inlines.contains(&Inline::Emph(vec![Inline::Str("italic".into())])));
        }
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn emphasis_after_realistic_leading_text_still_parses() {
    // The forward-scan budget never fires on valid input: long prose with a stray slash still parses.
    let lead = "The quick brown fox jumps over the lazy dog and/or cat. ".repeat(40);
    let input = format!("{lead}then *bold* and /italic/ close it out.");
    match &blocks(&input)[0] {
        Block::Para(inlines) => {
            assert!(inlines.contains(&Inline::Strong(vec![Inline::Str("bold".into())])));
            assert!(inlines.contains(&Inline::Emph(vec![Inline::Str("italic".into())])));
        }
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn headline_levels_and_ids() {
    let b = blocks("* First\n** Second");
    match &b[0] {
        Block::Header(1, attr, _) => assert_eq!(attr.id, "first"),
        other => panic!("expected header, got {other:?}"),
    }
    match &b[1] {
        Block::Header(2, attr, _) => assert_eq!(attr.id, "second"),
        other => panic!("expected header, got {other:?}"),
    }
}

#[test]
fn todo_keyword_and_tags() {
    let b = blocks("* TODO Task :work:");
    match &b[0] {
        Block::Header(1, attr, inlines) => {
            assert_eq!(attr.id, "task");
            assert!(
                matches!(inlines.first(), Some(Inline::Span(a, _)) if a.classes == ["todo", "TODO"])
            );
        }
        other => panic!("expected header, got {other:?}"),
    }
}

#[test]
fn src_block_becomes_code_block() {
    let b = blocks("#+BEGIN_SRC python\nprint(1)\n#+END_SRC");
    match &b[0] {
        Block::CodeBlock(attr, text) => {
            assert_eq!(attr.classes, ["python"]);
            assert_eq!(text, "print(1)\n");
        }
        other => panic!("expected code block, got {other:?}"),
    }
}

#[test]
fn bullet_and_ordered_lists() {
    assert!(
        matches!(blocks("- a\n- b").first(), Some(Block::BulletList(items)) if items.len() == 2)
    );
    assert!(matches!(
        blocks("1. a\n2. b").first(),
        Some(Block::OrderedList(..))
    ));
}

#[test]
fn definition_list() {
    match blocks("- term :: definition").first() {
        Some(Block::DefinitionList(entries)) => assert_eq!(entries.len(), 1),
        other => panic!("expected definition list, got {other:?}"),
    }
}

#[test]
fn link_and_image() {
    let b = blocks("[[https://example.com][label]] [[./x.png]]");
    match &b[0] {
        Block::Para(inlines) => {
            assert!(inlines.iter().any(|i| matches!(i, Inline::Link(..))));
            assert!(inlines.iter().any(|i| matches!(i, Inline::Image(..))));
        }
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn footnote_reference_resolves() {
    let b = blocks("Text[fn:1] more.\n\n[fn:1] The note.");
    match &b[0] {
        Block::Para(inlines) => {
            assert!(inlines.iter().any(|i| matches!(i, Inline::Note(_))));
        }
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn table_with_header() {
    match blocks("| a | b |\n|---+---|\n| 1 | 2 |").first() {
        Some(Block::Table(table)) => {
            assert_eq!(table.head.rows.len(), 1);
            assert_eq!(table.bodies.len(), 1);
        }
        other => panic!("expected table, got {other:?}"),
    }
}

#[test]
fn metadata_title() {
    let d = doc("#+TITLE: My Doc\n\nbody");
    assert!(d.meta.contains_key("title"));
}

#[test]
fn subscript_and_superscript() {
    let b = blocks("H_2O and x^2");
    match &b[0] {
        Block::Para(inlines) => {
            assert!(inlines.iter().any(|i| matches!(i, Inline::Subscript(_))));
            assert!(inlines.iter().any(|i| matches!(i, Inline::Superscript(_))));
        }
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn special_strings_dashes() {
    let b = blocks("em --- en -- dots ...");
    match &b[0] {
        Block::Para(inlines) => {
            let text = carta_ast::to_plain_text(inlines);
            assert!(text.contains('\u{2014}'));
            assert!(text.contains('\u{2013}'));
            assert!(text.contains('\u{2026}'));
        }
        other => panic!("expected paragraph, got {other:?}"),
    }
}

fn doc_with(input: &str, exts: &[Extension]) -> Document {
    let mut options = ReaderOptions::default();
    options.extensions = Extensions::from_list(exts);
    OrgReader.read(input, &options).unwrap()
}

#[test]
fn smart_quotes_and_apostrophe() {
    let d = doc_with("He said \"hi\" and it's 'fine'.", &[Extension::Smart]);
    let Block::Para(inlines) = &d.blocks[0] else {
        panic!("expected paragraph");
    };
    assert!(inlines.contains(&Inline::Quoted(
        QuoteType::DoubleQuote,
        vec![Inline::Str("hi".into())]
    )));
    assert!(inlines.contains(&Inline::Quoted(
        QuoteType::SingleQuote,
        vec![Inline::Str("fine".into())]
    )));
    assert!(inlines.contains(&Inline::Str("it\u{2019}s".into())));
}

#[test]
fn quotes_literal_without_smart() {
    let d = doc_with("say \"hi\".", &[]);
    let Block::Para(inlines) = &d.blocks[0] else {
        panic!("expected paragraph");
    };
    assert!(inlines.iter().all(|i| !matches!(i, Inline::Quoted(..))));
}

#[test]
fn gfm_and_ascii_identifiers() {
    let gfm = doc_with(
        "* Foo Bar 1.2",
        &[Extension::AutoIdentifiers, Extension::GfmAutoIdentifiers],
    );
    assert!(matches!(&gfm.blocks[0], Block::Header(_, a, _) if a.id == "foo-bar-12"));

    let ascii = doc_with(
        "* Café Résumé",
        &[Extension::AutoIdentifiers, Extension::AsciiIdentifiers],
    );
    assert!(matches!(&ascii.blocks[0], Block::Header(_, a, _) if a.id == "cafe-resume"));
}

#[test]
fn checkbox_literal_without_task_lists() {
    let d = doc_with("- [X] item", &[]);
    let Block::BulletList(items) = &d.blocks[0] else {
        panic!("expected bullet list");
    };
    let Block::Plain(inlines) = &items[0][0] else {
        panic!("expected plain");
    };
    assert!(inlines.contains(&Inline::Str("[X]".into())));
}

#[test]
fn entity_replacement() {
    let b = blocks("\\alpha and \\unknownentity");
    match &b[0] {
        Block::Para(inlines) => {
            assert!(carta_ast::to_plain_text(inlines).contains('α'));
            assert!(inlines.iter().any(|i| matches!(i, Inline::RawInline(..))));
        }
        other => panic!("expected paragraph, got {other:?}"),
    }
}
