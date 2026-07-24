//! Attribute, inline-code, and code-span inline-parse tests.

use super::*;

#[test]
fn bracketed_span_carries_attributes() {
    assert_eq!(
        pe("[text]{.cls #id}", exts(&[Extension::BracketedSpans])),
        vec![span(attr("id", &["cls"], &[]), vec![str("text")])]
    );
}

#[test]
fn empty_attribute_block_is_not_a_span() {
    assert_eq!(
        pe("[text]{}", exts(&[Extension::BracketedSpans])),
        vec![str("[text]{}")]
    );
}

#[test]
fn consecutive_attribute_blocks_merge_first_id_wins() {
    // Adjacent blocks accumulate classes and key/value pairs; the first identifier is kept.
    assert_eq!(
        pe(
            "[x]{#one .a}{#two .b k=v}",
            exts(&[Extension::BracketedSpans])
        ),
        vec![span(
            attr("one", &["a", "b"], &[("k", "v")]),
            vec![str("x")]
        )]
    );
}

#[test]
fn span_wins_over_shortcut_reference() {
    let refs = ref_map(&[("text", "http://r")]);
    let ext = exts(&[Extension::BracketedSpans]);
    assert_eq!(
        parse_inlines("[text]{.c}", &refs, no_notes(), ext),
        vec![span(attr("", &["c"], &[]), vec![str("text")])]
    );
}

#[test]
fn inline_code_takes_attributes() {
    assert_eq!(
        pe("`code`{.rust #x}", attrs()),
        vec![Inline::Code(
            Box::new(attr("x", &["rust"], &[])),
            "code".to_owned().into()
        )]
    );
    // A space before the block leaves it unattached (no wrapper artifact is produced).
    assert_eq!(
        pe("`code` x", attrs()),
        vec![
            Inline::Code(Box::default(), "code".to_owned().into()),
            Inline::Space,
            str("x")
        ]
    );
}

#[test]
fn link_and_image_take_attributes() {
    let link_with_attr = Inline::Link(
        Box::new(attr("home", &["external"], &[])),
        vec![str("t")],
        Box::new(Target {
            url: "u".to_owned().into(),
            title: carta_ast::Text::default(),
        }),
    );
    assert_eq!(pe("[t](u){.external #home}", attrs()), vec![link_with_attr]);
    let image_with_attr = Inline::Image(
        Box::new(attr("", &[], &[("width", "200")])),
        vec![str("a")],
        Box::new(Target {
            url: "i".to_owned().into(),
            title: carta_ast::Text::default(),
        }),
    );
    assert_eq!(pe("![a](i){width=200}", attrs()), vec![image_with_attr]);
}

#[test]
fn attributes_require_the_extension() {
    assert_eq!(p("[text]{.cls}"), vec![str("[text]{.cls}")]);
}

#[test]
fn nested_image_with_inner_link_and_deactivated_bracket() {
    // the inner link's success deactivates the `[` between `![` and `[foo]`; the next `]` must pop
    // and literalize that opener, not reach the image opener; only the final `](uri3)` closes it
    assert_eq!(
        p("![[[foo](uri1)](uri2)](uri3)"),
        vec![image(
            vec![str("["), link(vec![str("foo")], "uri1"), str("](uri2)"),],
            "uri3",
        )]
    );
}

#[test]
fn raw_attribute_turns_code_span_into_raw_inline() {
    let ext = exts(&[Extension::RawAttribute]);
    assert_eq!(pe("`<b>`{=html}", ext), vec![raw("html", "<b>")]);
    assert_eq!(pe("`\\x`{=latex}", ext), vec![raw("latex", "\\x")]);
}

#[test]
fn raw_attribute_format_token_allows_word_chars_dash_underscore() {
    let ext = exts(&[Extension::RawAttribute]);
    assert_eq!(pe("`x`{=my-format}", ext), vec![raw("my-format", "x")]);
    assert_eq!(pe("`x`{=my_fmt}", ext), vec![raw("my_fmt", "x")]);
    assert_eq!(pe("`x`{=3d}", ext), vec![raw("3d", "x")]);
}

#[test]
fn raw_attribute_tolerates_whitespace_around_marker() {
    let ext = exts(&[Extension::RawAttribute]);
    assert_eq!(pe("`x`{ =html }", ext), vec![raw("html", "x")]);
    assert_eq!(pe("`x`{=html }", ext), vec![raw("html", "x")]);
    assert_eq!(pe("`x`{ =html}", ext), vec![raw("html", "x")]);
}

#[test]
fn raw_attribute_normalizes_code_content() {
    let ext = exts(&[Extension::RawAttribute]);
    // A single space padding each side is stripped, exactly as for a code span.
    assert_eq!(pe("` x `{=html}", ext), vec![raw("html", "x")]);
}

#[test]
fn raw_attribute_requires_a_pure_format_marker() {
    let ext = exts(&[Extension::RawAttribute]);
    // A space between `=` and the format is not a marker.
    assert_eq!(
        pe("`x`{= html}", ext),
        vec![code("x"), str("{="), Inline::Space, str("html}"),]
    );
    // An empty format is not a marker.
    assert_eq!(pe("`x`{=}", ext), vec![code("x"), str("{=}")]);
    // Anything beyond the format (a class, a dot) defeats the marker.
    assert_eq!(pe("`x`{=a.b}", ext), vec![code("x"), str("{=a.b}")]);
}

#[test]
fn plain_attribute_block_on_code_span_is_not_raw() {
    // `{.class}` keeps the code span and applies the attribute (inline code attributes on).
    let ext = exts(&[Extension::RawAttribute, Extension::InlineCodeAttributes]);
    assert_eq!(
        pe("`x`{.c}", ext),
        vec![Inline::Code(
            Box::new(Attr {
                classes: vec!["c".to_owned().into()],
                ..Attr::default()
            }),
            "x".to_owned().into()
        )]
    );
}

#[test]
fn raw_attribute_off_leaves_marker_literal() {
    assert_eq!(p("`<b>`{=html}"), vec![code("<b>"), str("{=html}")]);
}

#[test]
fn code_span_matches_equal_length_closer() {
    assert_eq!(p("`a`"), vec![code("a")]);
}

#[test]
fn code_span_with_no_closer_stays_literal() {
    assert_eq!(p("`a"), vec![str("`a")]);
}

#[test]
fn code_span_failed_search_does_not_mask_a_different_length_match() {
    // the length-1 opener stays literal; the length-2 span after it must still match, since the
    // close index is keyed by run length and one length's absence does not suppress another's
    assert_eq!(p("`a ``b``"), vec![str("`a"), Inline::Space, code("b")]);
}

#[test]
fn code_span_opener_is_a_run_suffix_stays_literal() {
    // the escape eats the run's first backtick, so the opener is a run suffix whose length need
    // not equal any full run's; no length-2 run closes it, so both openers stay literal
    assert_eq!(
        p("\\``` x \\``` x"),
        vec![
            str("```"),
            Inline::Space,
            str("x"),
            Inline::Space,
            str("```"),
            Inline::Space,
            str("x"),
        ]
    );
}

#[test]
fn code_span_distinct_run_lengths_all_resolve() {
    // correctness face of the adversarial quadratic input: every opener literal, text between intact
    assert_eq!(
        p("`a ``b ```c"),
        vec![
            str("`a"),
            Inline::Space,
            str("``b"),
            Inline::Space,
            str("```c"),
        ]
    );
}

#[test]
fn code_span_close_before_cursor_is_not_reused() {
    // the close search must start at its own opener, never returning an already-consumed closer
    assert_eq!(p("`a` `b`"), vec![code("a"), Inline::Space, code("b")]);
}

#[test]
fn code_span_runs_at_buffer_ends_match() {
    // Opener at position 0 and closer as the final characters of the buffer.
    assert_eq!(p("``a``"), vec![code("a")]);
}

#[test]
fn code_span_index_matches_scan_on_tricky_buffers() {
    // nested lengths, adjacent runs of different lengths, and a shorter inner run before the matching closer
    assert_eq!(p("``x`y``"), vec![code("x`y")]);
    assert_eq!(p("`a ``b`` c`"), vec![code("a ``b`` c")]);
    assert_eq!(p("``a` b``"), vec![code("a` b")]);
}
