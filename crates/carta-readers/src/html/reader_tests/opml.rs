//! Tests for the inline-fragment parser used by the OPML reader.

use super::super::parse_inline_fragment;
use carta_ast::Inline;

#[cfg(feature = "opml")]
#[test]
fn inline_fragment_parses_markup_and_trims_edges() {
    let inlines = parse_inline_fragment("  <strong>a</strong> b <code>c</code>  ");
    assert_eq!(
        inlines,
        vec![
            Inline::Strong(vec![Inline::Str("a".to_string().into())]),
            Inline::Space,
            Inline::Str("b".to_string().into()),
            Inline::Space,
            Inline::Code(Box::default(), "c".to_string().into()),
        ]
    );
}

#[cfg(feature = "opml")]
#[test]
fn inline_fragment_resolves_character_references() {
    let inlines = parse_inline_fragment("a &amp; b");
    assert_eq!(
        inlines,
        vec![
            Inline::Str("a".to_string().into()),
            Inline::Space,
            Inline::Str("&".to_string().into()),
            Inline::Space,
            Inline::Str("b".to_string().into()),
        ]
    );
}

#[cfg(feature = "opml")]
fn raw(tag: &str) -> Inline {
    Inline::RawInline(
        carta_ast::Format("html".to_string().into()),
        tag.to_string().into(),
    )
}

#[cfg(feature = "opml")]
#[test]
fn inline_fragment_preserves_an_unrecognized_tag_verbatim() {
    let inlines = parse_inline_fragment("<cite>Book</cite>");
    assert_eq!(
        inlines,
        vec![
            raw("<cite>"),
            Inline::Str("Book".to_string().into()),
            raw("</cite>")
        ]
    );
}

#[cfg(feature = "opml")]
#[test]
fn inline_fragment_keeps_unknown_tag_attributes() {
    let inlines = parse_inline_fragment("<time datetime=\"2020\">y</time>");
    assert_eq!(
        inlines,
        vec![
            raw("<time datetime=\"2020\">"),
            Inline::Str("y".to_string().into()),
            raw("</time>"),
        ]
    );
}

#[cfg(feature = "opml")]
#[test]
fn inline_fragment_escapes_attribute_values_and_emits_bare_boolean() {
    let inlines = parse_inline_fragment("<x-foo a=\"1<2&3\" hidden>z</x-foo>");
    assert_eq!(
        inlines,
        vec![
            raw("<x-foo a=\"1&lt;2&amp;3\" hidden>"),
            Inline::Str("z".to_string().into()),
            raw("</x-foo>"),
        ]
    );
}

#[cfg(feature = "opml")]
#[test]
fn inline_fragment_lowercases_an_unknown_tag_name() {
    let inlines = parse_inline_fragment("<CITE>b</CITE>");
    assert_eq!(
        inlines,
        vec![
            raw("<cite>"),
            Inline::Str("b".to_string().into()),
            raw("</cite>")
        ]
    );
}

#[cfg(feature = "opml")]
#[test]
fn inline_fragment_void_unknown_tag_is_a_single_raw_inline() {
    let inlines = parse_inline_fragment("a <wbr> b");
    assert_eq!(
        inlines,
        vec![
            Inline::Str("a".to_string().into()),
            Inline::Space,
            raw("<wbr>"),
            Inline::Space,
            Inline::Str("b".to_string().into()),
        ]
    );
}

#[cfg(feature = "opml")]
#[test]
fn inline_fragment_self_closing_unknown_tag_pairs_open_and_close() {
    let inlines = parse_inline_fragment("<custom-tag/>");
    assert_eq!(inlines, vec![raw("<custom-tag>"), raw("</custom-tag>")]);
}

#[cfg(feature = "opml")]
#[test]
fn inline_fragment_unclosed_unknown_tag_omits_the_close() {
    let inlines = parse_inline_fragment("a <cite>open-only");
    assert_eq!(
        inlines,
        vec![
            Inline::Str("a".to_string().into()),
            Inline::Space,
            raw("<cite>"),
            Inline::Str("open-only".to_string().into()),
        ]
    );
}

#[cfg(feature = "opml")]
#[test]
fn inline_fragment_stray_unknown_end_tag_is_preserved() {
    let inlines = parse_inline_fragment("</cite> tail");
    assert_eq!(
        inlines,
        vec![
            raw("</cite>"),
            Inline::Space,
            Inline::Str("tail".to_string().into()),
        ]
    );
}

#[cfg(feature = "opml")]
#[test]
fn inline_fragment_unknown_tag_wraps_recognized_inner_markup() {
    let inlines = parse_inline_fragment("<cite><em>x</em></cite>");
    assert_eq!(
        inlines,
        vec![
            raw("<cite>"),
            Inline::Emph(vec![Inline::Str("x".to_string().into())]),
            raw("</cite>"),
        ]
    );
}

#[cfg(feature = "opml")]
#[test]
fn inline_fragment_recognized_tags_keep_structural_mapping() {
    let inlines = parse_inline_fragment("<em>e</em> <strong>s</strong> <sup>2</sup>");
    assert_eq!(
        inlines,
        vec![
            Inline::Emph(vec![Inline::Str("e".to_string().into())]),
            Inline::Space,
            Inline::Strong(vec![Inline::Str("s".to_string().into())]),
            Inline::Space,
            Inline::Superscript(vec![Inline::Str("2".to_string().into())]),
        ]
    );
}
