//! Auto-identifier and link and header reference tests.

use super::*;

#[test]
fn gfm_auto_identifiers_slug_headers_and_count_duplicates() {
    let result = blocks_with(
        "# Foo & Bar\n\n# 1.2 Section\n\n# Foo & Bar\n",
        Extension::GfmAutoIdentifiers,
    );
    // Punctuation drops without collapsing gaps; a repeated slug takes its occurrence count.
    assert_eq!(
        header_ids(&result),
        ["foo--bar", "12-section", "foo--bar-1"]
    );
}

#[test]
fn auto_identifiers_strip_leading_runs_and_increment_until_unique() {
    let result = blocks_with(
        "# 1. Intro\n\n# Intro\n\n# Intro\n",
        Extension::AutoIdentifiers,
    );
    // The leading non-letter run is stripped; each repeat increments until unused.
    assert_eq!(header_ids(&result), ["intro", "intro-1", "intro-2"]);
}

#[test]
fn auto_identifiers_fall_back_to_section_for_empty_slugs() {
    let result = blocks_with("# !!!\n\n# ???\n", Extension::AutoIdentifiers);
    assert_eq!(header_ids(&result), ["section", "section-1"]);
}

#[test]
fn auto_identifiers_off_leaves_headers_unidentified() {
    assert_eq!(header_ids(&blocks("# Hello World\n")), [""]);
}

#[test]
fn only_link_reference_definitions_leave_no_paragraph() {
    assert!(blocks("[a]: /one\n[b]: /two\n").is_empty());
}

#[test]
fn link_reference_definitions_strip_and_keep_the_body() {
    // Leading definitions are stripped; the shortcut reference resolves against them.
    let doc = blocks("[a]: /url\nSee [a] here.\n");
    assert!(
        matches!(doc.as_slice(), [Block::Para(_)]),
        "expected one paragraph, got {doc:?}"
    );
    assert_eq!(reference_targets(&doc), ["/url"]);
}

#[test]
fn an_unterminated_bracket_is_not_a_definition() {
    // A lone `[` that never forms `[label]:` is ordinary text and registers nothing.
    let doc = blocks("[not a definition\n");
    assert!(
        matches!(doc.as_slice(), [Block::Para(_)]),
        "expected one paragraph, got {doc:?}"
    );
    assert!(reference_targets(&doc).is_empty());
}

#[test]
fn a_leading_abbreviation_definition_is_consumed() {
    // A definition-only paragraph vanishes; a body after the definition remains.
    assert!(
        blocks_with(
            "*[HTML]: Hyper Text Markup Language\n",
            Extension::Abbreviations
        )
        .is_empty()
    );
    let doc = blocks_with(
        "*[HTML]: Hyper Text Markup Language\nHTML is old.\n",
        Extension::Abbreviations,
    );
    assert!(
        matches!(doc.as_slice(), [Block::Para(_)]),
        "expected one paragraph, got {doc:?}"
    );
}

#[test]
fn implicit_header_references_resolve_a_shortcut_reference() {
    let result = blocks_with_many("# Some Heading\n\n[Some Heading]\n", HEADER_REFS);
    assert_eq!(reference_targets(&result), ["#some-heading"]);
}

#[test]
fn implicit_header_references_match_full_collapsed_and_image_forms() {
    let result = blocks_with_many(
        "# Some Heading\n\n[text][Some Heading] [Some Heading][] ![Some Heading]\n",
        HEADER_REFS,
    );
    assert_eq!(
        reference_targets(&result),
        ["#some-heading", "#some-heading", "#some-heading"]
    );
}

#[test]
fn implicit_header_references_fold_case_and_collapse_whitespace() {
    let result = blocks_with_many("# Some Heading\n\n[SOME    HEADING]\n", HEADER_REFS);
    assert_eq!(reference_targets(&result), ["#some-heading"]);
}

#[test]
fn implicit_header_references_match_on_label_source_not_decoded_text() {
    // Labels match the heading's literal source, so the unmarked form does not resolve.
    let result = blocks_with_many(
        "# Heading with *emphasis*\n\n[Heading with *emphasis*] [Heading with emphasis]\n",
        HEADER_REFS,
    );
    assert_eq!(reference_targets(&result), ["#heading-with-emphasis"]);
}

#[test]
fn an_explicit_definition_outranks_an_implicit_header_reference() {
    let result = blocks_with_many(
        "# Linked Elsewhere\n\n[Linked Elsewhere]: https://example.com/x\n\n[Linked Elsewhere]\n",
        HEADER_REFS,
    );
    // An explicit definition with the same label is registered first and keeps the link.
    assert_eq!(reference_targets(&result), ["https://example.com/x"]);
}

#[test]
fn a_repeated_heading_is_reachable_only_through_the_first() {
    let result = blocks_with_many("# Twice\n\n# Twice\n\n[Twice]\n", HEADER_REFS);
    // The first occurrence keeps the bare identifier, so the reference resolves to it.
    assert_eq!(reference_targets(&result), ["#twice"]);
}

#[test]
fn implicit_header_references_resolve_before_their_heading() {
    let result = blocks_with_many("[Later Section]\n\n# Later Section\n", HEADER_REFS);
    assert_eq!(reference_targets(&result), ["#later-section"]);
}

#[test]
fn implicit_header_references_off_leaves_the_label_literal() {
    let result = blocks_with(
        "# Some Heading\n\n[Some Heading]\n",
        Extension::GfmAutoIdentifiers,
    );
    assert!(reference_targets(&result).is_empty());
    let [_, Block::Para(inlines)] = result.as_slice() else {
        panic!("expected a heading then a paragraph, got {result:?}");
    };
    assert!(
        inlines
            .iter()
            .any(|i| matches!(i, Inline::Str(s) if s.contains("[Some")))
    );
}

#[test]
fn implicit_header_references_plain_heading_matches_an_ordinary_paragraph_parse() {
    let result = blocks_with_many("# Simple title\n\nSimple title\n", HEADER_REFS);
    let [
        Block::Header(_, _, header_inlines),
        Block::Para(para_inlines),
    ] = result.as_slice()
    else {
        panic!("expected a heading then a paragraph, got {result:?}");
    };
    // No trigger character, so the pre-pass parse is reused; it still matches an ordinary parse.
    assert_eq!(header_inlines, para_inlines);
}

#[test]
fn implicit_header_references_heading_with_a_citation_is_not_cached() {
    let result = blocks_with_many(
        "# About @doe99\n\nSee @smith too.\n",
        &[
            Extension::GfmAutoIdentifiers,
            Extension::ImplicitHeaderReferences,
            Extension::Citations,
        ],
    );
    // `@` blocks pre-pass reuse: the body pass renumbers against the running citation count.
    assert_eq!(cite_note_nums(&result), [1, 2]);
}

#[test]
fn implicit_header_references_heading_with_a_footnote_resolves_in_the_body_pass() {
    let result = blocks_with_many(
        "# Title[^1]\n\n[^1]: the note body\n",
        &[
            Extension::GfmAutoIdentifiers,
            Extension::ImplicitHeaderReferences,
            Extension::Footnotes,
        ],
    );
    let [Block::Header(_, _, inlines)] = result.as_slice() else {
        panic!("expected a single heading, got {result:?}");
    };
    let note = inlines
        .iter()
        .find_map(|inline| match inline {
            Inline::Note(blocks) => Some(blocks.clone()),
            _ => None,
        })
        .expect("a note should be present");
    // `^` blocks pre-pass reuse: the body pass sees the real footnote body.
    assert!(matches!(note.as_slice(), [Block::Para(_)]));
}

#[test]
fn implicit_header_references_heading_referencing_a_later_heading_resolves_in_the_body_pass() {
    let result = blocks_with_many("# See [Later Heading]\n\n# Later Heading\n", HEADER_REFS);
    let [Block::Header(_, _, inlines), _] = result.as_slice() else {
        panic!("expected two headings, got {result:?}");
    };
    // `[` blocks pre-pass reuse: the body pass sees the full reference map.
    assert!(matches!(inlines.as_slice(), [.., Inline::Link(..)]));
}

#[test]
fn implicit_header_references_duplicate_headings_both_resolve_and_get_disambiguated_ids() {
    let result = blocks_with_many("# Dup\n\n# Dup\n", HEADER_REFS);
    assert_eq!(header_ids(&result), ["dup", "dup-1"]);
    let [Block::Header(_, _, first), Block::Header(_, _, second)] = result.as_slice() else {
        panic!("expected two headings, got {result:?}");
    };
    // Both occurrences pop their own queued parse and resolve identically.
    assert_eq!(first, second);
}
