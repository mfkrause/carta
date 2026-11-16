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

/// The inlines of `input` as a paragraph that follows another, in a compact notation: `S`trong,
/// `E`mph, stri`K`eout, `U`nderline, `C`ode, `L`ink, `N`ote, `Sub`/`Sup`, `_` for a space, `/` for
/// a soft break and `//` for a hard one.
fn shape(input: &str) -> String {
    opening_shape(&format!("lead\n\n{input}"), 1)
}

/// The inlines of the paragraph at `index` in the document `input`, in the notation of [`shape`].
fn opening_shape(input: &str, index: usize) -> String {
    fn inlines(inlines: &[Inline]) -> String {
        let parts: Vec<String> = inlines.iter().map(inline).collect();
        parts.join(" ")
    }
    fn inline(node: &Inline) -> String {
        match node {
            Inline::Str(text) => text.to_string(),
            Inline::Space => "_".to_owned(),
            Inline::SoftBreak => "/".to_owned(),
            Inline::LineBreak => "//".to_owned(),
            Inline::Strong(content) => format!("S[{}]", inlines(content)),
            Inline::Emph(content) => format!("E[{}]", inlines(content)),
            Inline::Strikeout(content) => format!("K[{}]", inlines(content)),
            Inline::Underline(content) => format!("U[{}]", inlines(content)),
            Inline::Subscript(content) => format!("Sub[{}]", inlines(content)),
            Inline::Superscript(content) => format!("Sup[{}]", inlines(content)),
            Inline::Code(_, text) => format!("C[{text}]"),
            Inline::Link(_, content, _) => format!("L[{}]", inlines(content)),
            Inline::Note(blocks) => match blocks.first() {
                Some(Block::Para(content)) => format!("N[{}]", inlines(content)),
                _ => "N[]".to_owned(),
            },
            other => format!("{other:?}"),
        }
    }
    match blocks(input).get(index) {
        Some(Block::Para(content)) => inlines(content),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn emphasis_reads_its_content_before_it_looks_for_its_close() {
    for (input, expected) in [
        ("*a *b* c*", "S[a _ S[b] _ c]"),
        ("*a /b *c* d/ e*", "S[a _ E[b _ S[c] _ d] _ e]"),
        // The inner span takes the only closing marker, so the outer one stays text.
        ("*a *b c*", "*a _ S[b _ c]"),
        ("*a =b* c= d*", "S[a _ C[b* c] _ d]"),
        ("*a [[http://x.y][b* c]] d*", "S[a _ L[b* _ c] _ d]"),
        ("*a^{b* c} d*", "S[a Sup[b* _ c] _ d]"),
        ("_a_b_", "U[a Sub[b]]"),
    ] {
        assert_eq!(shape(input), expected, "{input}");
    }
}

#[test]
fn a_marker_opens_emphasis_only_where_markup_may_open() {
    for (input, expected) in [
        ("*a*b*", "S[a*b]"),
        ("x *a*", "x _ S[a]"),
        ("x (*a*)", "x _ ( S[a] )"),
        ("x [*a*]", "x _ [*a*]"),
        ("x a*b*", "x _ a*b*"),
        // Directly after an opening marker.
        ("**a**", "S[S[a]]"),
        ("*/*/a/*/*", "S[E[S[E[a]]]]"),
        ("* a*", "* _ a*"),
    ] {
        assert_eq!(
            shape(&format!("x {input}")),
            format!("x _ {expected}"),
            "{input}"
        );
    }
}

#[test]
fn a_closing_marker_precedes_punctuation_or_the_enclosing_marker() {
    for (input, expected) in [
        ("*a*.", "S[a] ."),
        ("*a*b", "*a*b"),
        ("*a *", "*a _ *"),
        // The enclosing marker is the one directly around the span, not any further out.
        ("**a***", "S[S[a] *]"),
        ("*//a/*", "S[E[/a]]"),
        ("*a /b/* c*", "S[a _ E[b]] _ c*"),
        ("*a [[http://x.y][/b/*]]*", "S[a _ L[E[b] *]]"),
        ("*a ~b~* c*", "S[a _ C[b]] _ c*"),
    ] {
        assert_eq!(shape(input), expected, "{input}");
    }
}

#[test]
fn emphasis_whose_first_character_would_close_it_stays_text() {
    for (input, expected) in [
        ("a ** b*", "a _ ** _ b*"),
        ("a == b=", "a _ == _ b="),
        ("***", "S[*]"),
        ("******", "S[****]"),
        ("****a****", "S[***a***]"),
        ("*a ** b*", "S[a _ *] _ b*"),
    ] {
        assert_eq!(shape(input), expected, "{input}");
    }
}

#[test]
fn emphasis_holds_one_line_break_counted_from_the_last_opening_marker() {
    for (input, expected) in [
        ("*a\nb*", "S[a / b]"),
        ("*a\nb\nc*", "*a / b / c*"),
        ("*a /b\nc/ d\ne*", "*a _ E[b / c] _ d / e*"),
        ("*a\n/b/ c\nd*", "S[a / E[b] _ c / d]"),
        // Verbatim text counts its own line break.
        ("=a\nb\nc=", "=a / b / c="),
        ("*a =b\nc= d\ne*", "S[a _ C[b c] _ d / e]"),
    ] {
        assert_eq!(shape(input), expected, "{input}");
    }
}

#[test]
fn markup_opens_after_any_character_once_a_description_or_braced_script_ends() {
    for (input, expected) in [
        ("[[http://x.y][b]]x*c*", "L[b] x S[c]"),
        ("a^{b}x*c*", "a Sup[b] x S[c]"),
        ("a^{b}x y*c*", "a Sup[b] x _ y*c*"),
        ("a^{b}x-y*c*", "a Sup[b] x-y*c*"),
        ("[[http://x.y][a*b*]]", "L[a S[b]]"),
        ("[[http://x.y]]x*c*", "L[http://x.y] x*c*"),
        // A subscript binds to the word before it ahead of an underline.
        ("a^{b}x_c_", "a Sup[b] x Sub[c] _"),
    ] {
        assert_eq!(shape(input), expected, "{input}");
    }
}

#[test]
fn a_script_attaches_to_plain_text() {
    for (input, expected) in [
        ("a_b", "a Sub[b]"),
        ("a!_b", "a! Sub[b]"),
        ("a)_b", "a)_b"),
        ("*a*_b", "*a*_b"),
        ("x _b", "x _ _b"),
        // Not to the script before it.
        ("a_b_c", "a Sub[b] _c"),
        ("a^{b}_c", "a Sup[b] _c"),
        ("x_ay_b", "x Sub[ay] _b"),
        ("x_{a}y_b", "x Sub[a] y Sub[b]"),
    ] {
        assert_eq!(shape(input), expected, "{input}");
    }
}

#[test]
fn a_script_is_a_group_a_lone_star_or_signed_alphanumerics() {
    for (input, expected) in [
        ("a_1.5", "a Sub[1] .5"),
        ("a_i,j", "a Sub[i] ,j"),
        ("10^-3", "10 Sup[-3]"),
        ("a^+", "a^+"),
        ("a_*b*", "a Sub[*] b*"),
        ("a_{i,j}", "a Sub[i,j]"),
        ("a_{b{c}d}", "a Sub[b{c}d]"),
        // A parenthesized script keeps its parentheses and holds no line break.
        ("a_(b c)", "a Sub[(b _ c)]"),
        ("a_(b(c)d)e", "a Sub[(b(c)d)] e"),
        ("a_(b", "a_(b"),
        ("a_(b\nc)", "a_(b / c)"),
        ("a_(*b*)", "a Sub[( S[b] )]"),
        ("a_(x*c*)", "a Sub[(x*c*)]"),
        ("a_{x*c*}", "a Sub[x S[c]]"),
        ("a_(b)x*c*", "a Sub[(b)] x S[c]"),
    ] {
        assert_eq!(shape(input), expected, "{input}");
    }
}

#[test]
fn an_inline_footnote_sees_the_characters_around_its_text() {
    for (input, expected) in [
        ("a [fn::x *b* c]", "a _ N[x _ S[b] _ c]"),
        ("a [fn::x *b*]", "a _ N[x _ *b*]"),
        ("a [fn::*b* c]", "a _ N[*b* _ c]"),
        ("*a [fn::x /b/*]*", "S[a _ N[x _ E[b] *]]"),
    ] {
        assert_eq!(shape(input), expected, "{input}");
    }
}

#[test]
fn unclosed_markers_of_every_kind_parse_in_linear_time() {
    let input = "*a /b +c _d =e ~f ".repeat(2_000);
    assert!(shape(&input).starts_with("*a _ /b _ +c _ _d _ =e _ ~f"));
}

#[test]
fn emphasis_nested_far_past_the_native_stack_parses() {
    let depth = 20_000;
    let input = format!("{}core{}", "*/".repeat(depth), "/*".repeat(depth));
    let mut blocks = blocks(&input);
    let Some(Block::Para(inlines)) = blocks.first_mut() else {
        panic!("expected paragraph, got {blocks:?}");
    };
    let mut inlines = std::mem::take(inlines);
    let mut levels = 0;
    loop {
        let [Inline::Strong(inner) | Inline::Emph(inner)] = inlines.as_mut_slice() else {
            break;
        };
        inlines = std::mem::take(inner);
        levels += 1;
    }
    assert_eq!(levels, 2 * depth);
    assert_eq!(inlines, vec![Inline::Str("core".into())]);
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
fn a_letter_marks_a_list_item_only_with_fancy_lists() {
    assert_eq!(shape("x\nc) y"), "x / c) _ y");
    let mut options = ReaderOptions::default();
    options.extensions = Extensions::from_list(&[Extension::FancyLists]);
    let document = OrgReader.read("c) y", &options).unwrap();
    assert!(matches!(
        document.blocks.first(),
        Some(Block::OrderedList(..))
    ));
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

#[test]
fn file_links_preserve_urls_and_absolute_paths() {
    for (source, expected) in [
        ("[[file:///tmp/document.txt]]", "file:///tmp/document.txt"),
        (
            "[[file://server/share/document.txt]]",
            "file://server/share/document.txt",
        ),
        ("[[/tmp/document.txt]]", "file:///tmp/document.txt"),
        ("[[file:/tmp/image.png]]", "file:///tmp/image.png"),
        ("[[file:./image.png]]", "./image.png"),
    ] {
        let mut document = doc(source);
        let mut destinations = Vec::new();
        carta_core::walk::for_each_document_inline(&mut document, &mut |inline| {
            if let Inline::Link(_, _, target) | Inline::Image(_, _, target) = inline {
                destinations.push(target.url.to_string());
            }
        });
        assert_eq!(destinations, [expected], "{source}");
    }
}
