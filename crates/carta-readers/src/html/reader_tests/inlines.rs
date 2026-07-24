//! Tests for inline markup, headings, entities, and attribute parsing.

use super::super::HtmlReader;
use super::common::{blocks, first_block, para_inlines};
use carta_ast::{Block, Inline, Target};
use carta_core::{Reader, ReaderOptions};

#[test]
fn heading_generates_identifier() {
    let result = blocks("<h1>Hello World</h1>");
    let Some(Block::Header(level, attr, _)) = result.first() else {
        panic!("expected header");
    };
    assert_eq!(*level, 1);
    assert_eq!(attr.id, "hello-world");
}

#[test]
fn duplicate_identifiers_are_disambiguated() {
    let result = blocks("<h1>Sec</h1><h2>Sec</h2>");
    let ids: Vec<&str> = result
        .iter()
        .filter_map(|block| match block {
            Block::Header(_, attr, _) => Some(attr.id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(ids, vec!["sec", "sec-1"]);
}

#[test]
fn entities_are_decoded() {
    let result = blocks("<p>a &amp; b &copy; c</p>");
    let Some(Block::Para(inlines)) = result.first() else {
        panic!("expected paragraph");
    };
    assert!(inlines.contains(&Inline::Str("&".to_string().into())));
    assert!(inlines.contains(&Inline::Str("\u{a9}".to_string().into())));
}

#[test]
fn comment_joins_surrounding_text() {
    let result = blocks("<p>a<!-- c -->b</p>");
    let Some(Block::Para(inlines)) = result.first() else {
        panic!("expected paragraph");
    };
    assert_eq!(inlines.as_slice(), [Inline::Str("ab".to_string().into())]);
}

#[test]
fn script_content_is_dropped() {
    assert!(blocks("<script>var x = 1;</script><p>p</p>").len() == 1);
}

#[test]
fn head_metadata_is_extracted() {
    let document = HtmlReader
        .read(
            "<head><title>T</title><meta name=\"author\" content=\"A\"></head><body><p>b</p></body>",
            &ReaderOptions::default(),
        )
        .expect("reader should not fail");
    assert!(document.meta.contains_key("title"));
    assert!(document.meta.contains_key("author"));
}

#[test]
fn normalizes_crlf_and_strips_bom() {
    let inlines = para_inlines("\u{feff}<p>a\r\nb</p>");
    assert_eq!(
        inlines.as_slice(),
        [
            Inline::Str("a".to_string().into()),
            Inline::SoftBreak,
            Inline::Str("b".to_string().into())
        ]
    );
}

#[test]
fn every_inline_emphasis_kind_is_mapped() {
    let inlines = para_inlines(
        "<p><em>a</em><b>b</b><del>c</del><u>d</u><sup>e</sup><sub>f</sub><q>g</q></p>",
    );
    assert!(matches!(
        inlines.as_slice(),
        [
            Inline::Emph(_),
            Inline::Strong(_),
            Inline::Strikeout(_),
            Inline::Underline(_),
            Inline::Superscript(_),
            Inline::Subscript(_),
            Inline::Quoted(_, _),
        ]
    ));
}

#[test]
fn class_carrying_inlines_become_spans() {
    let inlines = para_inlines("<p><mark>m</mark><kbd>k</kbd></p>");
    let classes: Vec<&str> = inlines
        .iter()
        .filter_map(|inline| match inline {
            Inline::Span(attr, _) => attr.classes.first().map(carta_ast::Text::as_str),
            _ => None,
        })
        .collect();
    assert_eq!(classes, vec!["mark", "kbd"]);
}

#[test]
fn code_variants_force_classes() {
    let inlines = para_inlines("<p><code>c</code><samp>s</samp><var>v</var></p>");
    let classes: Vec<Vec<String>> = inlines
        .iter()
        .filter_map(|inline| match inline {
            Inline::Code(attr, _) => Some(attr.classes.iter().map(ToString::to_string).collect()),
            _ => None,
        })
        .collect();
    assert_eq!(
        classes,
        vec![
            Vec::<String>::new(),
            vec!["sample".to_string()],
            vec!["variable".to_string()],
        ]
    );
}

#[test]
fn line_break_element_becomes_line_break() {
    let inlines = para_inlines("<p>a<br>b</p>");
    assert!(inlines.contains(&Inline::LineBreak));
}

#[test]
fn anchor_with_href_is_a_link() {
    let inlines = para_inlines(r#"<p><a href="/u" title="T" class="x">t</a></p>"#);
    let Some(Inline::Link(attr, _, target)) = inlines.first() else {
        panic!("expected link");
    };
    assert_eq!(
        *target,
        Box::new(Target {
            url: "/u".to_string().into(),
            title: "T".to_string().into()
        })
    );
    assert!(attr.classes.contains(&"x".into()));
}

#[test]
fn anchor_with_name_is_a_span_with_id() {
    let inlines = para_inlines(r#"<p><a name="anchor">t</a></p>"#);
    let Some(Inline::Span(attr, _)) = inlines.first() else {
        panic!("expected span");
    };
    assert_eq!(attr.id, "anchor");
}

#[test]
fn image_reads_src_title_and_alt() {
    let inlines = para_inlines(r#"<p><img src="a.png" title="T" alt="alt text"></p>"#);
    let Some(Inline::Image(_, alt, target)) = inlines.first() else {
        panic!("expected image");
    };
    assert_eq!(target.url, "a.png");
    assert_eq!(target.title, "T");
    assert_eq!(
        alt.as_slice(),
        [
            Inline::Str("alt".to_string().into()),
            Inline::Space,
            Inline::Str("text".to_string().into())
        ]
    );
}

#[test]
fn unknown_inline_element_is_transparent() {
    let inlines = para_inlines("<p>a<bogus>b</bogus>c</p>");
    assert_eq!(inlines.as_slice(), [Inline::Str("abc".to_string().into())]);
}

#[test]
fn data_attributes_drop_their_prefix() {
    let Block::Div(attr, _) = first_block(r#"<div id="d" data-role="note">x</div>"#) else {
        panic!("expected div");
    };
    assert_eq!(attr.id, "d");
    assert!(
        attr.attributes
            .contains(&("role".to_string().into(), "note".to_string().into()))
    );
}

#[test]
fn boolean_and_unquoted_attributes_parse() {
    let Block::OrderedList(attrs, _) = first_block("<ol reversed start=5><li>a</li></ol>") else {
        panic!("expected ordered list");
    };
    assert_eq!(attrs.start, 5);
}

#[test]
fn numeric_and_named_references_decode() {
    let inlines = para_inlines("<p>&#65;&#x42;&#X43;&copy</p>");
    assert_eq!(
        inlines.as_slice(),
        [Inline::Str("ABC\u{a9}".to_string().into())]
    );
}

#[test]
fn unknown_entity_is_left_verbatim() {
    let inlines = para_inlines("<p>&notreal;</p>");
    assert_eq!(
        inlines.as_slice(),
        [Inline::Str("&notreal;".to_string().into())]
    );
}

#[test]
fn style_block_is_dropped() {
    assert!(blocks("<style>p { color: red }</style><p>x</p>").len() == 1);
}

#[test]
fn textarea_content_is_read_as_text() {
    let inlines = para_inlines("<p><textarea>typed &amp; ok</textarea></p>");
    assert!(
        inlines
            .iter()
            .any(|inline| matches!(inline, Inline::Str(s) if s.contains('&')))
    );
}

#[test]
fn cdata_reads_as_text_and_processing_instruction_is_dropped() {
    // CDATA contributes character data; a processing instruction contributes nothing
    let inlines = para_inlines("<p>a<![CDATA[ junk ]]><?pi here?>b</p>");
    assert_eq!(
        inlines.as_slice(),
        [
            Inline::Str("a".to_string().into()),
            Inline::Space,
            Inline::Str("junk".to_string().into()),
            Inline::Space,
            Inline::Str("b".to_string().into()),
        ]
    );
}

#[test]
fn doctype_declaration_is_skipped() {
    assert!(matches!(
        first_block("<!DOCTYPE html><p>x</p>"),
        Block::Para(_)
    ));
}

#[test]
fn stray_less_than_is_literal_text() {
    let inlines = para_inlines("<p>a < b</p>");
    assert!(
        inlines
            .iter()
            .any(|inline| matches!(inline, Inline::Str(s) if s.contains('<')))
    );
}

#[test]
fn self_closing_span_has_no_children() {
    let inlines = para_inlines("<p>a<span/>b</p>");
    assert!(
        inlines
            .iter()
            .any(|inline| matches!(inline, Inline::Span(_, children) if children.is_empty()))
    );
}

#[test]
fn explicit_id_on_heading_is_preserved() {
    let Block::Header(_, attr, _) = first_block(r#"<h2 id="custom">Title</h2>"#) else {
        panic!("expected header");
    };
    assert_eq!(attr.id, "custom");
}
