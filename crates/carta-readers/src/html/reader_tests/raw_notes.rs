//! Tests for style/script handling, footnotes, and raw HTML passthrough.

use super::common::{blocks, html_defaults, para_inlines, read_with};
use carta_ast::{Block, Inline, MathType};
use carta_core::{Extension, Extensions};

#[test]
fn inline_style_becomes_raw_html() {
    let inlines = para_inlines("<p>a<style>.x{}</style>b</p>");
    assert!(inlines.iter().any(|inline| matches!(
        inline,
        Inline::RawInline(format, text)
            if format.0 == "html" && text == "<style>.x{}</style>"
    )));
}

#[test]
fn leading_style_block_is_dropped() {
    assert!(matches!(
        blocks("<style>.x{}</style><p>x</p>").as_slice(),
        [Block::Para(_)]
    ));
}

#[test]
fn style_after_a_block_is_kept_as_a_raw_paragraph() {
    let result = blocks("<p>a</p>\n<style>.x{}</style>\n<p>b</p>");
    let [Block::Para(_), Block::Para(mid), Block::Para(_)] = result.as_slice() else {
        panic!("expected three paragraphs");
    };
    assert!(matches!(
        mid.as_slice(),
        [Inline::RawInline(format, text)]
            if format.0 == "html" && text == "<style>.x{}</style>"
    ));
}

#[test]
fn style_directly_adjacent_to_a_block_is_dropped() {
    assert!(matches!(
        blocks("<p>a</p><style>.x{}</style><p>b</p>").as_slice(),
        [Block::Para(_), Block::Para(_)]
    ));
}

#[test]
fn adjacent_styles_share_one_raw_paragraph() {
    let result = blocks("<p>a</p>\n<style>s1{}</style>\n<style>s2{}</style>\n<p>b</p>");
    let [_, Block::Para(mid), _] = result.as_slice() else {
        panic!("expected three paragraphs");
    };
    assert!(matches!(
        mid.as_slice(),
        [
            Inline::RawInline(f1, t1),
            Inline::SoftBreak,
            Inline::RawInline(f2, t2),
        ] if f1.0 == "html" && t1 == "<style>s1{}</style>"
            && f2.0 == "html" && t2 == "<style>s2{}</style>"
    ));
}

#[test]
fn math_script_becomes_inline_math() {
    let inlines = para_inlines(r#"<p><script type="math/tex">\D</script></p>"#);
    assert!(matches!(
        inlines.as_slice(),
        [Inline::Math(MathType::InlineMath, text)] if text == "\\D"
    ));
}

#[test]
fn display_math_script_becomes_display_math() {
    let inlines = para_inlines(r#"<p><script type="math/tex; mode=display">\D</script></p>"#);
    assert!(matches!(
        inlines.as_slice(),
        [Inline::Math(MathType::DisplayMath, _)]
    ));
}

#[test]
fn non_math_script_is_dropped() {
    assert!(blocks("<p><script>run()</script></p>").is_empty());
}

#[test]
fn note_reference_reconstructs_body_and_drops_container() {
    let result = blocks(concat!(
        "text<a href=\"#fn1\" class=\"footnote-ref\" role=\"doc-noteref\"><sup>1</sup></a>\n",
        "<section class=\"footnotes\" role=\"doc-endnotes\"><hr /><ol>",
        "<li id=\"fn1\"><p>the note",
        "<a href=\"#fnref1\" class=\"footnote-back\" role=\"doc-backlink\">\u{21a9}</a></p></li>",
        "</ol></section>",
    ));
    assert_eq!(
        result.as_slice(),
        [Block::Plain(vec![
            Inline::Str("text".to_string().into()),
            Inline::Note(vec![Block::Para(vec![
                Inline::Str("the".to_string().into()),
                Inline::Space,
                Inline::Str("note".to_string().into()),
            ])]),
        ])]
    );
}

#[test]
fn unmatched_note_reference_becomes_an_empty_note() {
    let result = blocks("text<a href=\"#missing\" role=\"doc-noteref\"><sup>1</sup></a>");
    assert_eq!(
        result.as_slice(),
        [Block::Plain(vec![
            Inline::Str("text".to_string().into()),
            Inline::Note(Vec::new()),
        ])]
    );
}

/// Read with the `epub`-style dialect: the `html` defaults plus `raw_html`, which preserves
/// unknown tags, comments, and script/style bodies verbatim and lifts MathML into math.
fn read_raw_html(input: &str) -> Vec<Block> {
    read_with(
        input,
        html_defaults().union(Extensions::from_list(&[Extension::RawHtml])),
    )
}

fn raw_block(format: &carta_ast::Format, text: &carta_ast::Text) -> (String, String) {
    (format.0.to_string(), text.to_string())
}

#[test]
fn raw_html_lifts_mathml_to_inline_math() {
    let result = read_raw_html(
        r#"<p><math xmlns="http://www.w3.org/1998/Math/MathML"><msup><mi>x</mi><mn>2</mn></msup></math></p>"#,
    );
    let Some(Block::Para(inlines)) = result.first() else {
        panic!("expected paragraph");
    };
    assert_eq!(
        inlines.as_slice(),
        [Inline::Math(
            MathType::InlineMath,
            "x^{2}".to_string().into()
        )]
    );
}

#[test]
fn raw_html_reads_display_math_from_the_block_attribute() {
    let result = read_raw_html(
        r#"<p><math display="block" xmlns="http://www.w3.org/1998/Math/MathML"><mi>x</mi></math></p>"#,
    );
    let Some(Block::Para(inlines)) = result.first() else {
        panic!("expected paragraph");
    };
    assert!(matches!(
        inlines.as_slice(),
        [Inline::Math(MathType::DisplayMath, text)] if text == "x"
    ));
}

#[test]
fn mathml_is_unwrapped_without_raw_html() {
    let result =
        blocks(r#"<p><math xmlns="http://www.w3.org/1998/Math/MathML"><mi>x</mi></math></p>"#);
    let Some(Block::Para(inlines)) = result.first() else {
        panic!("expected paragraph");
    };
    assert_eq!(inlines.as_slice(), [Inline::Str("x".to_string().into())]);
}

#[test]
fn raw_html_comment_breaks_the_text_run() {
    let result = read_raw_html("<p>a<!-- c -->b</p>");
    let Some(Block::Para(inlines)) = result.first() else {
        panic!("expected paragraph");
    };
    assert_eq!(
        inlines.as_slice(),
        [
            Inline::Str("a".to_string().into()),
            Inline::RawInline(
                carta_ast::Format("html".to_string().into()),
                "<!-- c -->".to_string().into()
            ),
            Inline::Str("b".to_string().into()),
        ]
    );
}

#[test]
fn raw_html_wraps_a_block_level_unknown_element() {
    let result = read_raw_html("<article><p>x</p></article>");
    let [
        Block::RawBlock(of, open),
        Block::Para(_),
        Block::RawBlock(cf, close),
    ] = result.as_slice()
    else {
        panic!("expected a raw-wrapped article");
    };
    assert_eq!(raw_block(of, open), ("html".into(), "<article>".into()));
    assert_eq!(raw_block(cf, close), ("html".into(), "</article>".into()));
}

#[test]
fn raw_html_keeps_an_unknown_inline_element_verbatim() {
    let result = read_raw_html(r#"<p>x<custom-tag data-k="v">y</custom-tag>z</p>"#);
    let Some(Block::Para(inlines)) = result.first() else {
        panic!("expected paragraph");
    };
    assert_eq!(
        inlines.as_slice(),
        [
            Inline::Str("x".to_string().into()),
            Inline::RawInline(
                carta_ast::Format("html".to_string().into()),
                "<custom-tag data-k=\"v\">".to_string().into()
            ),
            Inline::Str("y".to_string().into()),
            Inline::RawInline(
                carta_ast::Format("html".to_string().into()),
                "</custom-tag>".to_string().into()
            ),
            Inline::Str("z".to_string().into()),
        ]
    );
}

fn raw_inline(text: &str) -> Inline {
    Inline::RawInline(carta_ast::Format("html".into()), text.into())
}

fn raw_html_block(text: &str) -> Block {
    Block::RawBlock(carta_ast::Format("html".into()), text.into())
}

#[test]
fn raw_html_preserves_orphan_inline_end_tags() {
    let result = read_raw_html("<p>a</i>b</span>c<br></br>d</br>e</img></p>");
    let [Block::Para(inlines)] = result.as_slice() else {
        panic!("expected a paragraph");
    };
    assert_eq!(
        inlines.as_slice(),
        [
            Inline::Str("a".into()),
            raw_inline("</i>"),
            Inline::Str("b".into()),
            raw_inline("</span>"),
            Inline::Str("c".into()),
            Inline::LineBreak,
            Inline::Str("d".into()),
            raw_inline("</br>"),
            Inline::Str("e".into()),
            raw_inline("</img>"),
        ]
    );
}

#[test]
fn raw_html_consumes_only_adjacent_self_closing_tags() {
    assert_eq!(read_raw_html("<hr></hr>"), [Block::HorizontalRule]);
    assert!(matches!(
        read_raw_html("<p><img src=\"x\"></img></p>").as_slice(),
        [Block::Para(inlines)] if matches!(inlines.as_slice(), [Inline::Image(_, _, target)] if target.url == "x")
    ));
}

#[test]
fn raw_html_preserves_end_tags_after_explicit_self_closing_tags() {
    assert_eq!(
        read_raw_html("<hr/></hr>"),
        [Block::HorizontalRule, raw_html_block("</hr>"),]
    );
    assert!(matches!(
        read_raw_html("<p><img src=\"x\"/></img></p>").as_slice(),
        [Block::Para(inlines)] if matches!(inlines.as_slice(), [
            Inline::Image(_, _, target),
            Inline::RawInline(format, text),
        ] if target.url == "x" && format.0 == "html" && text == "</img>")
    ));
}

#[test]
fn raw_html_normalizes_declarations_and_processing_instructions() {
    assert_eq!(
        read_raw_html("<!>"),
        [Block::Plain(vec![raw_inline("<!---->")])]
    );
    assert_eq!(
        read_raw_html("<!foo a=b>"),
        [raw_html_block("<!FOO a=\"b\">")]
    );
    assert_eq!(
        read_raw_html("<?foo a=b?>"),
        [raw_html_block("<?foo a=\"b\" ?>")]
    );
    assert_eq!(
        read_raw_html("<!bogus/>"),
        [raw_html_block("<!BOGUS>"), raw_html_block("</!bogus>"),]
    );
    assert!(read_raw_html("<?XML version=\"1.0\"?>").is_empty());
    assert_eq!(
        read_raw_html("<?3>"),
        [Block::Plain(vec![Inline::Str("<?3>".into())])]
    );
}

#[test]
fn raw_html_repairs_unclosed_comment_tokens() {
    for (source, expected) in [
        ("<!--", "<!---->"),
        ("<!--x>", "<!--x>-->"),
        ("<!--x--!", "<!--x-->"),
        ("<!--x--!\n", "<!--x--!\n-->"),
    ] {
        assert_eq!(
            read_raw_html(source),
            [Block::Plain(vec![raw_inline(expected)])],
            "{source:?}"
        );
    }
}

#[test]
fn html_without_raw_html_drops_declarations_comments_and_processing_instructions() {
    assert_eq!(
        blocks("<!foo><?foo><!-- x --><?3>"),
        [Block::Plain(vec![Inline::Str("<?3>".into())])]
    );
}

#[test]
fn raw_html_preserves_orphan_block_end_tags() {
    let result = read_raw_html("</p></div></article></meta></script></style></dt></tr>");
    let expected = [
        "</p>",
        "</div>",
        "</article>",
        "</meta>",
        "</script>",
        "</style>",
        "</dt>",
        "</tr>",
    ];
    assert_eq!(result, expected.map(raw_html_block));
}

#[test]
fn html_without_raw_html_drops_orphan_end_tags() {
    assert!(blocks("</i></p></div></meta></script></style></br>").is_empty());
}

#[test]
fn html_without_raw_html_drops_void_closers() {
    for source in ["<p><br></br>x</p>", "<p><br/>x</br></p>"] {
        assert_eq!(
            para_inlines(source),
            [Inline::LineBreak, Inline::Str("x".into())]
        );
    }
    for source in ["<hr></hr>", "<hr/></hr>"] {
        assert_eq!(blocks(source), [Block::HorizontalRule]);
    }
    for source in [
        "<p><img src=\"x\"></img></p>",
        "<p><img src=\"x\"/></img></p>",
    ] {
        assert!(matches!(
            para_inlines(source).as_slice(),
            [Inline::Image(_, _, target)] if target.url == "x"
        ));
    }
}

#[test]
fn raw_html_splits_text_at_orphan_document_end_tags() {
    for tag in ["head", "body", "html"] {
        assert_eq!(
            read_raw_html(&format!("before </{tag}> after")),
            [
                Block::Plain(vec![Inline::Str("before".into())]),
                Block::Plain(vec![Inline::Str("after".into())]),
            ],
            "{tag}"
        );
    }
}

#[test]
fn raw_html_falls_back_for_tables_without_cells() {
    for (source, tags) in [
        ("<table></table>", vec!["<table>", "</table>"]),
        (
            "<table><tr></tr></table>",
            vec!["<table>", "<tr>", "</tr>", "</table>"],
        ),
    ] {
        assert_eq!(
            read_raw_html(source),
            tags.into_iter().map(raw_html_block).collect::<Vec<_>>(),
            "{source}"
        );
    }

    let caption = read_raw_html("<table><caption>Caption</caption></table>");
    assert!(matches!(
        caption.as_slice(),
        [
            Block::RawBlock(_, _),
            Block::RawBlock(_, _),
            Block::Plain(text),
            Block::RawBlock(_, _),
            Block::RawBlock(_, _)
        ] if text == &[Inline::Str("Caption".into())]
    ));
}

#[test]
fn html_without_raw_html_falls_back_for_tables_without_cells() {
    assert!(blocks("<table></table>").is_empty());
    assert!(blocks("<table><tr></tr></table>").is_empty());
    assert_eq!(
        blocks("<table><caption>Caption</caption></table>"),
        [Block::Plain(vec![Inline::Str("Caption".into())])]
    );
}

#[test]
fn tables_with_empty_cells_remain_semantic_tables() {
    assert!(matches!(
        read_raw_html("<table><tr><td></td></tr></table>").as_slice(),
        [Block::Table(_)]
    ));
    assert!(matches!(
        blocks("<table><tr><td></td></tr></table>").as_slice(),
        [Block::Table(_)]
    ));
}

#[test]
fn raw_html_unwraps_a_grouping_main_element() {
    let result = read_raw_html("<main><p>m</p></main>");
    assert!(matches!(result.as_slice(), [Block::Para(_)]));
}

#[test]
fn raw_html_keeps_a_non_math_script_as_a_raw_block() {
    let result = read_raw_html("<script>run()</script><p>p</p>");
    let [Block::RawBlock(f, text), Block::Para(_)] = result.as_slice() else {
        panic!("expected a raw script block then a paragraph");
    };
    assert_eq!(
        raw_block(f, text),
        ("html".into(), "<script>run()</script>".into())
    );
}

#[test]
fn raw_html_keeps_a_leading_style_as_a_raw_block() {
    let result = read_raw_html("<style>.x{}</style><p>p</p>");
    let [Block::RawBlock(f, text), Block::Para(_)] = result.as_slice() else {
        panic!("expected a raw style block then a paragraph");
    };
    assert_eq!(
        raw_block(f, text),
        ("html".into(), "<style>.x{}</style>".into())
    );
}
