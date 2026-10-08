//! Emphasis and strong inline-parse tests.

use super::*;

// Emphasis and strong

#[test]
fn nested_emphasis_and_strong() {
    assert_eq!(
        p("*a **b** c*"),
        vec![Inline::Emph(vec![
            str("a"),
            Inline::Space,
            Inline::Strong(vec![str("b")]),
            Inline::Space,
            str("c"),
        ])]
    );
}

#[test]
fn mixed_asterisk_and_underscore() {
    assert_eq!(
        p("*a _b_ c*"),
        vec![Inline::Emph(vec![
            str("a"),
            Inline::Space,
            Inline::Emph(vec![str("b")]),
            Inline::Space,
            str("c"),
        ])]
    );
}

#[test]
fn triple_asterisk_produces_emph_of_strong() {
    assert_eq!(
        p("***a***"),
        vec![Inline::Emph(vec![Inline::Strong(vec![str("a")])])]
    );
}

#[test]
fn rule_of_3_prevents_outer_strong() {
    // A sum of three fails the rule of three unless both run lengths are multiples of three.
    assert_eq!(p("**a*b**"), vec![Inline::Strong(vec![str("a*b")])]);
}

#[test]
fn rule_of_3_prevents_inner_strong() {
    assert_eq!(p("*a**b*"), vec![Inline::Emph(vec![str("a**b")])]);
}

#[test]
fn unmatched_openers_become_literal() {
    assert_eq!(p("*a"), vec![str("*a")]);
    assert_eq!(p("a*"), vec![str("a*")]);
    assert_eq!(p("**a*"), vec![str("*"), Inline::Emph(vec![str("a")])]);
}

#[test]
fn underscore_intraword_stays_literal() {
    // `_` between word chars cannot open or close (spec §6.3 rules).
    assert_eq!(p("a_b_c"), vec![str("a_b_c")]);
    assert_eq!(p("_a_b"), vec![str("_a_b")]);
}

#[test]
fn emphasis_flanks_across_multi_byte_neighbors() {
    assert_eq!(
        p("α*β*γ"),
        vec![str("α"), Inline::Emph(vec![str("β")]), str("γ")]
    );
}

#[test]
fn emphasis_with_multi_byte_content_at_input_edges() {
    // opener at input start, closer at input end: boundary lookups run against the buffer edges
    assert_eq!(p("*β*"), vec![Inline::Emph(vec![str("β")])]);
}

#[test]
fn emphasis_between_emoji_neighbors() {
    assert_eq!(
        p("😀*a*😀"),
        vec![str("😀"), Inline::Emph(vec![str("a")]), str("😀")]
    );
}

#[test]
fn underscore_intraword_stays_literal_with_multi_byte_neighbors() {
    // The word-character test before and after a `_` run reads whole characters, not bytes.
    assert_eq!(p("α_β_γ"), vec![str("α_β_γ")]);
}

#[test]
fn empty_input_parses_to_nothing() {
    assert_eq!(p(""), Vec::new());
}
