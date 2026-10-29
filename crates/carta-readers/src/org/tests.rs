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
fn emphasis_after_unmatched_openers_still_parses() {
    let lead = "/a ".repeat(1000);
    let input = format!("{lead}then *bold*.");
    match &blocks(&input)[0] {
        Block::Para(inlines) => {
            assert!(inlines.contains(&Inline::Strong(vec![Inline::Str("bold".into())])));
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

#[test]
fn heading_properties_choose_custom_ids_before_fallback_ids() {
    for (properties, expected) in [
        (":ID: stable", "stable"),
        (":ID: first\n:ID: second", "first"),
        (":ID: stable\n:CUSTOM_ID: custom", "custom"),
        (":ID: stable\n:CUSTOM_ID: ", "heading"),
    ] {
        let source = format!("* Heading\n:PROPERTIES:\n{properties}\n:END:\n");
        let Block::Header(_, attributes, _) = &blocks(&source)[0] else {
            panic!()
        };
        assert_eq!(attributes.id.as_str(), expected);
    }
}

#[test]
fn file_properties_keep_literal_metadata_and_first_values() {
    let document = doc(
        "# Comment\n:PROPERTIES:\n:ID: file-id\n:FOO: **first**\n:FOO: second\n:TITLE: Original\n:SPACE: value\u{a0}\n:FOO\u{a0}BAR: nonbreaking key\n:END:\n#+TITLE: Replacement\n\nText.\n",
    );
    assert_eq!(
        document.meta.get("id"),
        Some(&MetaValue::MetaString("file-id".into()))
    );
    assert_eq!(
        document.meta.get("foo"),
        Some(&MetaValue::MetaString("**first**".into()))
    );
    assert_eq!(
        document.meta.get("title"),
        Some(&MetaValue::MetaString("Original".into()))
    );
    assert_eq!(
        document.meta.get("space"),
        Some(&MetaValue::MetaString("value\u{a0}".into()))
    );
    assert_eq!(
        document.meta.get("foo\u{a0}bar"),
        Some(&MetaValue::MetaString("nonbreaking key".into()))
    );
    assert_eq!(
        document.blocks,
        vec![Block::Para(vec![Inline::Str("Text.".into())])]
    );
}

#[test]
fn unfinished_drawers_keep_following_headlines() {
    assert_eq!(
        blocks(":DRAWER:\ntext\n"),
        vec![Block::Para(vec![
            Inline::Str(":DRAWER:".into()),
            Inline::SoftBreak,
            Inline::Str("text".into()),
        ])]
    );
    let source = "* Heading\n:PROPERTIES:\n:ID: unfinished\n".repeat(1000);
    let document = doc(&source);
    assert_eq!(
        document
            .blocks
            .iter()
            .filter(|block| matches!(block, Block::Header(..)))
            .count(),
        1000
    );
    assert_eq!(document.blocks.len(), 2000);
}

#[test]
fn heading_properties_keep_classes_and_raw_attributes() {
    let source = "* Heading\n:PROPERTIES:\n:ID: stable\n:CLASS: primary secondary\n:UNNUMBERED: t\n:FOO: *value*\n:FOO: second\n:END:\n";
    let Block::Header(_, attributes, _) = &blocks(source)[0] else {
        panic!()
    };
    assert_eq!(attributes.classes, ["primary", "secondary", "unnumbered"]);
    assert_eq!(
        attributes.attributes,
        vec![
            ("foo".into(), "*value*".into()),
            ("foo".into(), "second".into())
        ]
    );
}

#[test]
fn named_links_resolve_forward_without_changing_file_links() {
    let mut document = doc("[[target][*label*]] [[file:target]]\n\n<<target>>\n");
    let mut destinations = Vec::new();
    carta_core::walk::for_each_document_inline(&mut document, &mut |inline| {
        if let Inline::Link(_, _, target) = inline {
            destinations.push(target.url.to_string());
        }
    });
    assert_eq!(destinations, ["#target", "target"]);
    let Block::Para(inlines) = &document.blocks[0] else {
        panic!()
    };
    let Inline::Link(_, label, _) = &inlines[0] else {
        panic!()
    };
    assert_eq!(
        label,
        &vec![Inline::Strong(vec![Inline::Str("label".into())])]
    );
}

#[test]
fn normalized_targets_keep_distinct_working_links() {
    let mut document = doc("<<a/b>> <<a+b>>\n\n[[a/b]] [[a+b]]\n");
    let mut identifiers = Vec::new();
    let mut destinations = Vec::new();
    carta_core::walk::for_each_document_inline(&mut document, &mut |inline| match inline {
        Inline::Span(attributes, _) if !attributes.id.is_empty() => {
            identifiers.push(attributes.id.to_string());
        }
        Inline::Link(_, _, target) => destinations.push(target.url.to_string()),
        _ => {}
    });
    assert_eq!(identifiers, ["a-b", "a-b-1"]);
    assert_eq!(destinations, ["#a-b", "#a-b-1"]);
}

#[test]
fn unresolved_links_keep_their_labels() {
    let paragraph = blocks("[[missing][*label*]]");
    let Block::Para(inlines) = &paragraph[0] else {
        panic!()
    };
    let Inline::Span(attributes, children) = &inlines[0] else {
        panic!()
    };
    assert_eq!(attributes.classes, ["spurious-link"]);
    assert_eq!(
        attributes.attributes,
        vec![("target".into(), "missing".into())]
    );
    assert_eq!(
        children,
        &vec![Inline::Emph(vec![Inline::Strong(vec![Inline::Str(
            "label".into()
        )])])]
    );
    assert_eq!(
        blocks("[[missing][]]"),
        vec![Block::Para(vec![Inline::Str("[[missing][]]".into())])]
    );
}
