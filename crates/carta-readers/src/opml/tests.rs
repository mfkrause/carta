use super::*;

fn read(input: &str) -> Document {
    OpmlReader
        .read(input, &ReaderOptions::default())
        .expect("outline input parses")
}

fn headers(document: &Document) -> Vec<(i32, String)> {
    document
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Header(level, _, inlines) => Some((*level, inline_text(inlines))),
            _ => None,
        })
        .collect()
}

fn inline_text(inlines: &[Inline]) -> String {
    inlines
        .iter()
        .map(|inline| match inline {
            Inline::Str(text) => text.as_str(),
            Inline::Space => " ",
            _ => "",
        })
        .collect()
}

#[test]
fn nesting_assigns_header_levels() {
    let document = read(
        "<opml><body>\
             <outline text=\"A\">\
             <outline text=\"B\"><outline text=\"C\"/></outline>\
             </outline>\
             </body></opml>",
    );
    assert_eq!(
        headers(&document),
        [
            (1, "A".to_owned()),
            (2, "B".to_owned()),
            (3, "C".to_owned()),
        ]
    );
}

#[test]
fn sibling_outlines_share_a_level() {
    let document = read("<opml><body><outline text=\"A\"/><outline text=\"B\"/></body></opml>");
    assert_eq!(
        headers(&document),
        [(1, "A".to_owned()), (1, "B".to_owned())]
    );
}

#[test]
fn note_attribute_parses_as_markdown() {
    let document = read("<opml><body><outline text=\"H\" _note=\"**b**\"/></body></opml>");
    assert!(matches!(
        document.blocks.first(),
        Some(Block::Header(1, _, _))
    ));
    let Some(Block::Para(inlines)) = document.blocks.get(1) else {
        panic!("expected the note to parse into a paragraph");
    };
    assert!(matches!(inlines.first(), Some(Inline::Strong(_))));
}

#[test]
fn text_attribute_tokenizes_on_whitespace() {
    let document = read("<opml><body><outline text=\"Hello   World\"/></body></opml>");
    let Some(Block::Header(_, _, inlines)) = document.blocks.first() else {
        panic!("expected a header");
    };
    assert!(matches!(
        inlines.as_slice(),
        [Inline::Str(first), Inline::Space, Inline::Str(second)]
            if first == "Hello" && second == "World"
    ));
}

fn first_header_inlines(input: &str) -> Vec<Inline> {
    let document = read(input);
    match document.blocks.into_iter().next() {
        Some(Block::Header(_, _, inlines)) => inlines,
        _ => panic!("expected a header"),
    }
}

fn outline(text: &str) -> String {
    format!("<opml><body><outline text=\"{text}\"/></body></opml>")
}

#[test]
fn text_attribute_parses_inline_html_markup() {
    let inlines = first_header_inlines(&outline(
        "&lt;strong&gt;Bold&lt;/strong&gt; and &lt;em&gt;it&lt;/em&gt;",
    ));
    assert_eq!(
        inlines,
        vec![
            Inline::Strong(vec![Inline::Str("Bold".to_owned().into())]),
            Inline::Space,
            Inline::Str("and".to_owned().into()),
            Inline::Space,
            Inline::Emph(vec![Inline::Str("it".to_owned().into())]),
        ]
    );
}

#[test]
fn text_attribute_decodes_entities_twice_then_parses_code() {
    // the XML layer decodes once (`&amp;amp;` → `&amp;`); the inline parse decodes again and reads `<code>`
    let inlines = first_header_inlines(&outline("a &lt;code&gt;c&lt;/code&gt; b &amp;amp; z"));
    assert_eq!(
        inlines,
        vec![
            Inline::Str("a".to_owned().into()),
            Inline::Space,
            Inline::Code(Box::default(), "c".to_owned().into()),
            Inline::Space,
            Inline::Str("b".to_owned().into()),
            Inline::Space,
            Inline::Str("&".to_owned().into()),
            Inline::Space,
            Inline::Str("z".to_owned().into()),
        ]
    );
}

#[test]
fn text_attribute_parses_nested_markup() {
    let inlines = first_header_inlines(&outline(
        "&lt;strong&gt;&lt;em&gt;both&lt;/em&gt;&lt;/strong&gt;",
    ));
    assert_eq!(
        inlines,
        vec![Inline::Strong(vec![Inline::Emph(vec![Inline::Str(
            "both".to_owned().into()
        )])])]
    );
}

#[test]
fn text_attribute_parses_superscript_and_subscript() {
    let inlines = first_header_inlines(&outline(
        "x&lt;sup&gt;2&lt;/sup&gt;&lt;sub&gt;n&lt;/sub&gt;",
    ));
    assert_eq!(
        inlines,
        vec![
            Inline::Str("x".to_owned().into()),
            Inline::Superscript(vec![Inline::Str("2".to_owned().into())]),
            Inline::Subscript(vec![Inline::Str("n".to_owned().into())]),
        ]
    );
}

#[test]
fn text_attribute_parses_an_anchor_into_a_link() {
    let inlines = first_header_inlines(&outline(
        "&lt;a href=&quot;http://e.com&quot;&gt;l&lt;/a&gt;",
    ));
    let Some(Inline::Link(_, label, target)) = inlines.first() else {
        panic!("expected a link");
    };
    assert_eq!(label, &vec![Inline::Str("l".to_owned().into())]);
    assert_eq!(target.url, "http://e.com");
}

#[test]
fn named_character_reference_in_text_decodes_once_decoded() {
    // `&amp;copy;` survives the XML decode as `&copy;`, which the inline parse turns into ©.
    let inlines = first_header_inlines(&outline("c &amp;copy; r"));
    assert_eq!(
        inlines,
        vec![
            Inline::Str("c".to_owned().into()),
            Inline::Space,
            Inline::Str("\u{a9}".to_owned().into()),
            Inline::Space,
            Inline::Str("r".to_owned().into()),
        ]
    );
}

#[test]
fn link_outline_wraps_heading_in_a_link_to_its_url() {
    let document = read(
        "<opml><body><outline type=\"link\" text=\"Site\" url=\"http://e.com/p\"/></body></opml>",
    );
    let Some(Block::Header(1, _, inlines)) = document.blocks.first() else {
        panic!("expected a header");
    };
    let Some(Inline::Link(_, label, target)) = inlines.first() else {
        panic!("expected a link heading");
    };
    assert_eq!(label, &vec![Inline::Str("Site".to_owned().into())]);
    assert_eq!(target.url, "http://e.com/p");
    assert_eq!(target.title, "");
}

#[test]
fn link_outline_without_url_links_to_an_empty_target() {
    let document = read("<opml><body><outline type=\"LINK\" text=\"Site\"/></body></opml>");
    let Some(Block::Header(_, _, inlines)) = document.blocks.into_iter().next() else {
        panic!("expected a header");
    };
    let Some(Inline::Link(_, _, target)) = inlines.first() else {
        panic!("expected a link heading");
    };
    assert_eq!(target.url, "");
}

#[test]
fn non_link_outline_with_a_url_keeps_a_plain_heading() {
    let document =
        read("<opml><body><outline text=\"Site\" url=\"http://e.com/p\"/></body></opml>");
    let Some(Block::Header(_, _, inlines)) = document.blocks.first() else {
        panic!("expected a header");
    };
    assert_eq!(inlines.as_slice(), [Inline::Str("Site".to_owned().into())]);
}

#[test]
fn missing_text_attribute_yields_an_empty_heading() {
    let document = read("<opml><body><outline/></body></opml>");
    assert_eq!(headers(&document), [(1, String::new())]);
}

#[test]
fn single_quoted_attributes_are_read() {
    let document = read("<opml><body><outline text='quoted'/></body></opml>");
    assert_eq!(headers(&document), [(1, "quoted".to_owned())]);
}

#[test]
fn comments_instructions_and_doctype_are_skipped() {
    let document = read(
        "<?xml version=\"1.0\"?><!DOCTYPE opml><opml><!-- c -->\
             <body><outline text=\"A\"/></body></opml>",
    );
    assert_eq!(headers(&document), [(1, "A".to_owned())]);
}

#[test]
fn metadata_is_drawn_from_the_head() {
    let document = read(
        "<opml><head><title>T</title><ownerName>Me</ownerName>\
             <dateModified>2020</dateModified></head><body></body></opml>",
    );
    assert!(matches!(
        document.meta.get("title"),
        Some(MetaValue::MetaInlines(inlines)) if inline_text(inlines) == "T"
    ));
    assert!(matches!(
        document.meta.get("date"),
        Some(MetaValue::MetaInlines(inlines)) if inline_text(inlines) == "2020"
    ));
    let Some(MetaValue::MetaList(authors)) = document.meta.get("author") else {
        panic!("expected an author list");
    };
    assert!(matches!(
        authors.first(),
        Some(MetaValue::MetaInlines(inlines)) if inline_text(inlines) == "Me"
    ));
}

#[test]
fn absent_owner_yields_an_empty_author_list() {
    let document = read("<opml><head><title>T</title></head><body></body></opml>");
    assert!(matches!(
        document.meta.get("author"),
        Some(MetaValue::MetaList(authors)) if authors.is_empty()
    ));
}

#[test]
fn named_entities_decode() {
    assert_eq!(
        decode_entities("a &amp; b &lt;c&gt; &quot;d&quot; &apos;e&apos;"),
        "a & b <c> \"d\" 'e'"
    );
}

#[test]
fn numeric_entities_decode_in_decimal_and_hex() {
    assert_eq!(decode_entities("&#65;&#x42;&#X43;"), "ABC");
}

#[test]
fn malformed_or_unknown_references_are_left_verbatim() {
    assert_eq!(decode_entities("&amp"), "&amp");
    assert_eq!(decode_entities("&nosuch;"), "&nosuch;");
    assert_eq!(decode_entities("&#zz;"), "&#zz;");
    assert_eq!(decode_entities("bare & text"), "bare & text");
}

#[test]
fn malformed_markup_does_not_panic() {
    let _ = read("<opml><body><outline text=\"x\"><outline text=\"y\"></body>");
    let _ = read("<<<>>><opml attr");
    let _ = read("");
}

fn title_inlines(document: &Document) -> Vec<Inline> {
    match document.meta.get("title") {
        Some(MetaValue::MetaInlines(inlines)) => inlines.clone(),
        _ => panic!("expected title inlines"),
    }
}

#[test]
fn text_attribute_pairs_double_quotes_into_a_quoted_span() {
    let inlines = first_header_inlines(&outline("&quot;hi&quot;"));
    assert_eq!(
        inlines,
        vec![Inline::Quoted(
            QuoteType::DoubleQuote,
            vec![Inline::Str("hi".to_owned().into())]
        )]
    );
}

#[test]
fn text_attribute_pairs_single_quotes_into_a_quoted_span() {
    let inlines = first_header_inlines(&outline("&apos;hi&apos;"));
    assert_eq!(
        inlines,
        vec![Inline::Quoted(
            QuoteType::SingleQuote,
            vec![Inline::Str("hi".to_owned().into())]
        )]
    );
}

#[test]
fn text_attribute_curls_an_apostrophe() {
    let inlines = first_header_inlines(&outline("it&apos;s"));
    assert_eq!(inlines, vec![Inline::Str("it\u{2019}s".to_owned().into())]);
}

#[test]
fn text_attribute_folds_dashes_and_ellipsis() {
    let inlines = first_header_inlines(&outline("a---b--c...d"));
    // Three hyphens fold to an em dash, two to an en dash, three dots to an ellipsis.
    assert_eq!(
        inlines,
        vec![Inline::Str(
            "a\u{2014}b\u{2013}c\u{2026}d".to_owned().into()
        )]
    );
}

#[test]
fn dash_runs_fold_greedily_to_em_dashes() {
    assert_eq!(fold_dash_run_greedy(1), "-");
    assert_eq!(fold_dash_run_greedy(2), "\u{2013}");
    assert_eq!(fold_dash_run_greedy(3), "\u{2014}");
    assert_eq!(fold_dash_run_greedy(4), "\u{2014}-");
    assert_eq!(fold_dash_run_greedy(5), "\u{2014}\u{2013}");
    assert_eq!(fold_dash_run_greedy(6), "\u{2014}\u{2014}");
    assert_eq!(fold_dash_run_greedy(7), "\u{2014}\u{2014}-");
}

#[test]
fn ellipsis_runs_fold_per_group_of_three() {
    assert_eq!(fold_ellipsis_run(1), ".");
    assert_eq!(fold_ellipsis_run(2), "..");
    assert_eq!(fold_ellipsis_run(3), "\u{2026}");
    assert_eq!(fold_ellipsis_run(4), "\u{2026}.");
    assert_eq!(fold_ellipsis_run(6), "\u{2026}\u{2026}");
}

#[test]
fn text_attribute_resolves_an_unpaired_double_quote_directionally() {
    // an opener-context quote before a word becomes the left glyph; otherwise the right
    let opener = first_header_inlines(&outline("&quot;open only"));
    assert_eq!(
        opener.first(),
        Some(&Inline::Str("\u{201c}open".to_owned().into()))
    );
    let closer = first_header_inlines(&outline("close only&quot;"));
    assert_eq!(
        closer.last(),
        Some(&Inline::Str("only\u{201d}".to_owned().into()))
    );
}

#[test]
fn double_quotes_do_not_nest_within_their_own_kind() {
    // The inner double quote closes the outer span rather than nesting; the rest stay glyphs.
    let inlines = first_header_inlines(&outline("&quot;a &quot;b&quot; c&quot;"));
    assert_eq!(
        inlines,
        vec![
            Inline::Quoted(
                QuoteType::DoubleQuote,
                vec![Inline::Str("a".to_owned().into()), Inline::Space]
            ),
            Inline::Str("b\u{201d}".to_owned().into()),
            Inline::Space,
            Inline::Str("c\u{201d}".to_owned().into()),
        ]
    );
}

#[test]
fn a_different_quote_kind_nests() {
    let inlines = first_header_inlines(&outline("&quot;a &apos;b&apos; c&quot;"));
    assert_eq!(
        inlines,
        vec![Inline::Quoted(
            QuoteType::DoubleQuote,
            vec![
                Inline::Str("a".to_owned().into()),
                Inline::Space,
                Inline::Quoted(
                    QuoteType::SingleQuote,
                    vec![Inline::Str("b".to_owned().into())]
                ),
                Inline::Space,
                Inline::Str("c".to_owned().into()),
            ]
        )]
    );
}

#[test]
fn two_straight_single_quotes_stay_apostrophes() {
    let inlines = first_header_inlines(&outline("&apos;&apos;"));
    assert_eq!(
        inlines,
        vec![Inline::Str("\u{2019}\u{2019}".to_owned().into())]
    );
}

#[test]
fn code_span_curls_quotes_into_glyph_pairs() {
    let inlines = first_header_inlines(&outline("&lt;code&gt;&apos;q&apos;&lt;/code&gt;"));
    // A matched pair inside a code span renders as its left and right glyphs, not a Quoted node.
    assert_eq!(
        inlines,
        vec![Inline::Code(
            Box::default(),
            "\u{2018}q\u{2019}".to_owned().into()
        )]
    );
}

#[test]
fn code_span_curls_an_apostrophe_and_folds_dashes() {
    let inlines = first_header_inlines(&outline("&lt;code&gt;it&apos;s --- x&lt;/code&gt;"));
    assert_eq!(
        inlines,
        vec![Inline::Code(
            Box::default(),
            "it\u{2019}s \u{2014} x".to_owned().into()
        )]
    );
}

#[test]
fn smart_typography_recurses_into_inline_markup() {
    let inlines = first_header_inlines(&outline("&lt;em&gt;&quot;hi&quot;&lt;/em&gt;"));
    assert_eq!(
        inlines,
        vec![Inline::Emph(vec![Inline::Quoted(
            QuoteType::DoubleQuote,
            vec![Inline::Str("hi".to_owned().into())]
        )])]
    );
}

#[test]
fn note_body_uses_the_markdown_preset() {
    // a definition list proves the note body uses the extended Markdown extension set
    let document =
        read("<opml><body><outline text=\"H\" _note=\"Term&#10;:   Definition\"/></body></opml>");
    assert!(
        document
            .blocks
            .iter()
            .any(|block| matches!(block, Block::DefinitionList(_))),
        "expected the note to parse a definition list"
    );
}

#[test]
fn note_body_applies_smart_typography() {
    let document = read("<opml><body><outline text=\"H\" _note=\"it&apos;s\"/></body></opml>");
    let Some(Block::Para(inlines)) = document.blocks.get(1) else {
        panic!("expected a note paragraph");
    };
    assert_eq!(inlines, &vec![Inline::Str("it\u{2019}s".to_owned().into())]);
}

#[test]
fn metadata_keeps_straight_quotes_dashes_and_dots() {
    // Document metadata is not smart-transformed: its punctuation stays verbatim.
    let document = read(
        "<opml><head><title>&quot;a&quot; --- it&apos;s ...</title></head><body></body></opml>",
    );
    assert_eq!(
        title_inlines(&document),
        vec![
            Inline::Str("\"a\"".to_owned().into()),
            Inline::Space,
            Inline::Str("---".to_owned().into()),
            Inline::Space,
            Inline::Str("it's".to_owned().into()),
            Inline::Space,
            Inline::Str("...".to_owned().into()),
        ]
    );
}

#[test]
fn metadata_preserves_boundary_whitespace_as_space() {
    let document = read("<opml><head><title>  a b  </title></head><body></body></opml>");
    assert_eq!(
        title_inlines(&document),
        vec![
            Inline::Space,
            Inline::Str("a".to_owned().into()),
            Inline::Space,
            Inline::Str("b".to_owned().into()),
            Inline::Space,
        ]
    );
}

#[test]
fn metadata_turns_an_internal_newline_into_a_soft_break() {
    let document = read("<opml><head><title>line one\nline two</title></head><body></body></opml>");
    assert_eq!(
        title_inlines(&document),
        vec![
            Inline::Str("line".to_owned().into()),
            Inline::Space,
            Inline::Str("one".to_owned().into()),
            Inline::SoftBreak,
            Inline::Str("line".to_owned().into()),
            Inline::Space,
            Inline::Str("two".to_owned().into()),
        ]
    );
}

#[test]
fn present_but_empty_owner_contributes_an_empty_author() {
    // a present `ownerName`, even empty, yields one author entry; an absent element yields none
    let document = read("<opml><head><ownerName></ownerName></head><body></body></opml>");
    let Some(MetaValue::MetaList(authors)) = document.meta.get("author") else {
        panic!("expected an author list");
    };
    assert_eq!(authors, &vec![MetaValue::MetaInlines(Vec::new())]);
}
