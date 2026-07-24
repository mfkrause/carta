//! Link and image inline-parse tests.

use super::*;

// --- Links and images ---

#[test]
fn inline_link_and_image() {
    assert_eq!(p("[a](u)"), vec![link(vec![str("a")], "u")]);
    assert_eq!(p("![i](u)"), vec![image(vec![str("i")], "u")]);
}

#[test]
fn unmatched_image_opener_keeps_its_bang() {
    // An image opener that never finds a closing `]` reverts to the literal `![`, not `[`.
    assert_eq!(p("![x"), vec![str("![x")]);
    assert_eq!(p("![[a]x"), vec![str("![[a]x")]);
}

#[test]
fn reference_link_with_and_without_ref() {
    assert_eq!(p("[a][r]"), vec![str("[a][r]")]);
    let refs = ref_map(&[("r", "http://r")]);
    let result = parse_inlines("[a][r]", &refs, no_notes(), no_ext());
    assert_eq!(result, vec![link(vec![str("a")], "http://r")]);
}

#[test]
fn shortcut_reference_resolves_only_with_a_matching_definition() {
    assert_eq!(p("[foo]"), vec![str("[foo]")]);
    // A matching definition resolves the shortcut, with case folding on the label.
    let refs = ref_map(&[("foo", "http://f")]);
    assert_eq!(
        parse_inlines("[Foo]", &refs, no_notes(), no_ext()),
        vec![link(vec![str("Foo")], "http://f")]
    );
}

#[test]
fn shortcut_label_near_the_length_bound_still_resolves() {
    // 999 chars sits under the byte guard; the guard only skips spans too long to be a label
    let label = "a".repeat(999);
    let refs = ref_map(&[(label.as_str(), "http://f")]);
    let source = format!("[{label}]");
    assert_eq!(
        parse_inlines(&source, &refs, no_notes(), no_ext()),
        vec![link(vec![str(&label)], "http://f")]
    );
}

#[test]
fn span_past_the_label_bound_never_resolves_as_shortcut_or_collapsed() {
    // a span past MAX_LABEL_BYTES is no label even when an oversized definition key matches
    let oversized = "a".repeat(super::super::MAX_LABEL_BYTES + 1);
    let refs = ref_map(&[(oversized.as_str(), "http://big")]);
    let shortcut = format!("[{oversized}]");
    assert_eq!(
        parse_inlines(&shortcut, &refs, no_notes(), no_ext()),
        vec![str(&shortcut)]
    );
    let collapsed = format!("[{oversized}][]");
    assert_eq!(
        parse_inlines(&collapsed, &refs, no_notes(), no_ext()),
        vec![str(&collapsed)]
    );
    // The same collapsed form under the bound resolves through the identical path.
    let refs = ref_map(&[("foo", "http://f")]);
    assert_eq!(
        parse_inlines("[Foo][]", &refs, no_notes(), no_ext()),
        vec![link(vec![str("Foo")], "http://f")]
    );
}

#[test]
fn footnote_reference_resolves_at_the_bracket_boundary() {
    let mut defined = BTreeSet::new();
    defined.insert("x".to_owned());
    let mut by_id: BTreeMap<String, Vec<Block>> = BTreeMap::new();
    by_id.insert("x".to_owned(), vec![Block::Para(vec![str("note")])]);
    let examples = ExampleMap::new();
    let cite = Cell::new(0);
    let notes = RefContext {
        defined: &defined,
        by_id: &by_id,
        in_definition: false,
        markdown: false,
        examples: &examples,
        cite_count: &cite,
    };
    let ext = exts(&[Extension::Footnotes]);
    assert_eq!(
        parse_inlines("[^x]", &empty_refs(), notes, ext),
        vec![Inline::Note(vec![Block::Para(vec![str("note")])])]
    );
    assert_eq!(pe("[^x]", ext), vec![str("[^x]")]);
}

#[test]
fn spaced_reference_link_allows_whitespace_before_the_label() {
    let refs = ref_map(&[("ref", "http://r"), ("text", "http://t")]);
    let ext = exts(&[Extension::SpacedReferenceLinks]);
    // display comes from the first bracket, target from the second, separated by space or newline
    assert_eq!(
        parse_inlines("[text] [ref]", &refs, no_notes(), ext),
        vec![link(vec![str("text")], "http://r")]
    );
    assert_eq!(
        parse_inlines("[text]\n[ref]", &refs, no_notes(), ext),
        vec![link(vec![str("text")], "http://r")]
    );
    // An empty second bracket is a collapsed reference keyed on the first bracket.
    assert_eq!(
        parse_inlines("[text] []", &refs, no_notes(), ext),
        vec![link(vec![str("text")], "http://t")]
    );
    // an undefined second label leaves the whole run literal: the text is not retried as a shortcut
    let only_text = ref_map(&[("text", "http://t")]);
    assert_eq!(
        parse_inlines("[text] [ref]", &only_text, no_notes(), ext),
        vec![str("[text]"), Inline::Space, str("[ref]")]
    );
    // Without the extension the space breaks the pair into two shortcut references.
    assert_eq!(
        parse_inlines("[text] [ref]", &refs, no_notes(), no_ext()),
        vec![
            link(vec![str("text")], "http://t"),
            Inline::Space,
            link(vec![str("ref")], "http://r"),
        ]
    );
}

#[test]
fn nested_bracket_in_link_text() {
    // [[a]](u): the inner [a] has no target of its own, so it stays literal in the link text
    assert_eq!(p("[[a]](u)"), vec![link(vec![str("[a]")], "u")]);
}

#[test]
fn unmatched_brackets_are_literal() {
    assert_eq!(p("]]]"), vec![str("]]]")]);
}

#[test]
fn link_suppresses_earlier_bracket_openers() {
    // [a [b](u) c](v): the inner link deactivates the outer opener (no link may contain a link), so `[a ` and `](v)` stay literal
    assert_eq!(
        p("[a [b](u) c](v)"),
        vec![
            str("[a"),
            Inline::Space,
            link(vec![str("b")], "u"),
            Inline::Space,
            str("c](v)"),
        ]
    );
}

#[test]
fn emphasis_inside_link_text() {
    assert_eq!(
        p("[*a*](u)"),
        vec![link(vec![Inline::Emph(vec![str("a")])], "u")]
    );
}
