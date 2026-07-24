//! Native-span and mark inline-parse tests.

use super::*;

#[test]
fn native_span_carries_id_class_and_pairs() {
    assert_eq!(
        pe(
            r#"<span id="i" class="a b" data-x="y">hi *there*</span>"#,
            native()
        ),
        vec![span(
            attr("i", &["a", "b"], &[("data-x", "y")]),
            vec![str("hi"), Inline::Space, Inline::Emph(vec![str("there")])]
        )]
    );
}

#[test]
fn native_span_without_attributes() {
    assert_eq!(
        pe("a <span>x</span> b", native()),
        vec![
            str("a"),
            Inline::Space,
            span(attr("", &[], &[]), vec![str("x")]),
            Inline::Space,
            str("b"),
        ]
    );
}

#[test]
fn native_span_empty_content() {
    assert_eq!(
        pe("<span></span>", native()),
        vec![span(attr("", &[], &[]), vec![])]
    );
}

#[test]
fn native_span_nests_innermost_first() {
    assert_eq!(
        pe(
            r#"<span class="o"><span class="i">x</span></span>"#,
            native()
        ),
        vec![span(
            attr("", &["o"], &[]),
            vec![span(attr("", &["i"], &[]), vec![str("x")])]
        )]
    );
}

#[test]
fn native_span_tag_name_is_case_insensitive() {
    assert_eq!(
        pe(r#"<SPAN class="a">x</SPAN>"#, native()),
        vec![span(attr("", &["a"], &[]), vec![str("x")])]
    );
}

#[test]
fn native_span_keeps_non_span_tags_raw() {
    // An unrelated tag inside a span stays raw inline HTML.
    assert_eq!(
        pe(r#"<span class="a">x <b>y</b></span>"#, native()),
        vec![span(
            attr("", &["a"], &[]),
            vec![
                str("x"),
                Inline::Space,
                raw("html", "<b>"),
                str("y"),
                raw("html", "</b>"),
            ]
        )]
    );
}

#[test]
fn native_span_attribute_values_and_booleans() {
    // Single-quoted, unquoted, and valueless attributes; a duplicate id/class keeps the first.
    assert_eq!(
        pe("<span data-x='y z'>q</span>", native()),
        vec![span(attr("", &[], &[("data-x", "y z")]), vec![str("q")])]
    );
    assert_eq!(
        pe("<span flag>q</span>", native()),
        vec![span(attr("", &[], &[("flag", "")]), vec![str("q")])]
    );
    assert_eq!(
        pe(
            r#"<span id="a" id="b" class="c" class="d">q</span>"#,
            native()
        ),
        vec![span(attr("a", &["c"], &[]), vec![str("q")])]
    );
}

#[test]
fn native_span_decodes_entities_in_attribute_values() {
    assert_eq!(
        pe(r#"<span title="a &amp; b">q</span>"#, native()),
        vec![span(attr("", &[], &[("title", "a & b")]), vec![str("q")])]
    );
}

#[test]
fn native_span_self_closing_stays_raw() {
    // `<span/>` has no content to wrap.
    assert_eq!(
        pe("a <span/> b", native()),
        vec![
            str("a"),
            Inline::Space,
            raw("html", "<span/>"),
            Inline::Space,
            str("b"),
        ]
    );
}

#[test]
fn native_span_unclosed_opener_reverts_to_raw() {
    assert_eq!(
        pe(r#"<span class="a">no close"#, native()),
        vec![
            raw("html", "<span class=\"a\">"),
            str("no"),
            Inline::Space,
            str("close"),
        ]
    );
}

#[test]
fn native_span_pairs_inside_emphasis() {
    assert_eq!(
        pe("*x <span>y</span> z*", native()),
        vec![Inline::Emph(vec![
            str("x"),
            Inline::Space,
            span(attr("", &[], &[]), vec![str("y")]),
            Inline::Space,
            str("z"),
        ])]
    );
}

#[test]
fn native_span_off_leaves_tags_raw() {
    assert_eq!(
        p(r#"<span class="a">x</span>"#),
        vec![
            raw("html", "<span class=\"a\">"),
            str("x"),
            raw("html", "</span>"),
        ]
    );
}

#[test]
fn mark_wraps_a_double_equals_run() {
    let on = exts(&[Extension::Mark]);
    assert_eq!(
        pe("a ==x== b", on),
        vec![
            str("a"),
            Inline::Space,
            mark(vec![str("x")]),
            Inline::Space,
            str("b"),
        ]
    );
}

#[test]
fn mark_resolves_inner_emphasis() {
    let on = exts(&[Extension::Mark]);
    assert_eq!(
        pe("==x *y*==", on),
        vec![mark(vec![
            str("x"),
            Inline::Space,
            Inline::Emph(vec![str("y")]),
        ])]
    );
}

#[test]
fn mark_off_leaves_double_equals_literal() {
    assert_eq!(
        pe("a ==x== b", no_ext()),
        vec![
            str("a"),
            Inline::Space,
            str("==x=="),
            Inline::Space,
            str("b"),
        ]
    );
}

#[test]
fn mark_opener_needs_no_following_space() {
    let on = exts(&[Extension::Mark]);
    // A space just inside either delimiter blocks the run; both sides stay literal.
    assert_eq!(pe("== x==", on), vec![str("=="), Inline::Space, str("x==")]);
    assert_eq!(pe("==x ==", on), vec![str("==x"), Inline::Space, str("==")]);
}

#[test]
fn mark_lone_equals_stays_literal() {
    let on = exts(&[Extension::Mark]);
    assert_eq!(
        pe("a = b", on),
        vec![str("a"), Inline::Space, str("="), Inline::Space, str("b")]
    );
}

#[test]
fn mark_run_pairs_once_and_leaves_excess_literal() {
    let on = exts(&[Extension::Mark]);
    // four-on-four pairs only the innermost two per side; the outer `==` never re-pair into a nested mark
    assert_eq!(
        pe("====x====", on),
        vec![str("=="), mark(vec![str("x")]), str("==")]
    );
    // Two-on-four consumes two from each, leaving the surplus `==` literal.
    assert_eq!(pe("==x====", on), vec![mark(vec![str("x")]), str("==")]);
}
