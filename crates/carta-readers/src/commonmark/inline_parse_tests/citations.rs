//! Citation and heading-cache inline-parse tests.

use super::*;

#[test]
fn bare_citation_is_author_in_text() {
    assert_eq!(
        pe("@doe2020", cites()),
        vec![cite(
            vec![citation(
                "doe2020",
                vec![],
                vec![],
                CitationMode::AuthorInText,
                1
            )],
            vec![str("@doe2020")],
        )]
    );
}

#[test]
fn bare_citation_needs_a_non_word_before_the_at() {
    // Glued to a preceding word, the `@` is literal: no citation, no email autolink here.
    assert_eq!(pe("foo@bar", cites()), vec![str("foo@bar")]);
    // A space before the `@` lets it open a citation.
    assert_eq!(
        pe("a @b", cites()),
        vec![
            str("a"),
            Inline::Space,
            cite(
                vec![citation("b", vec![], vec![], CitationMode::AuthorInText, 1)],
                vec![str("@b")],
            ),
        ]
    );
}

#[test]
fn bracket_citation_carries_prefix_and_suffix() {
    assert_eq!(
        pe("[see @doe2020 and more]", cites()),
        vec![cite(
            vec![citation(
                "doe2020",
                vec![str("see")],
                vec![Inline::Space, str("and"), Inline::Space, str("more")],
                CitationMode::NormalCitation,
                1,
            )],
            vec![
                str("[see"),
                Inline::Space,
                str("@doe2020"),
                Inline::Space,
                str("and"),
                Inline::Space,
                str("more]"),
            ],
        )]
    );
}

#[test]
fn dash_before_at_suppresses_author() {
    assert_eq!(
        pe("[-@k]", cites()),
        vec![cite(
            vec![citation(
                "k",
                vec![],
                vec![],
                CitationMode::SuppressAuthor,
                1
            )],
            vec![str("[-@k]")],
        )]
    );
    // A `-` glued to a preceding word is part of the prefix, not a suppression marker.
    assert_eq!(
        pe("[a-@b]", cites()),
        vec![cite(
            vec![citation(
                "b",
                vec![str("a-")],
                vec![],
                CitationMode::NormalCitation,
                1
            )],
            vec![str("[a-@b]")],
        )]
    );
}

#[test]
fn semicolon_separates_entries_sharing_one_number() {
    assert_eq!(
        pe("[@a; @b]", cites()),
        vec![cite(
            vec![
                citation("a", vec![], vec![], CitationMode::NormalCitation, 1),
                citation("b", vec![], vec![], CitationMode::NormalCitation, 1),
            ],
            vec![str("[@a;"), Inline::Space, str("@b]")],
        )]
    );
}

#[test]
fn comma_nests_a_bare_citation_in_the_suffix() {
    // `@b` after a comma is a bare citation inside `a`'s suffix; the group takes the higher number
    assert_eq!(
        pe("[@a, @b]", cites()),
        vec![cite(
            vec![citation(
                "a",
                vec![],
                vec![
                    str(","),
                    Inline::Space,
                    cite(
                        vec![citation("b", vec![], vec![], CitationMode::AuthorInText, 2)],
                        vec![str("@b")],
                    ),
                ],
                CitationMode::NormalCitation,
                2,
            )],
            vec![str("[@a,"), Inline::Space, str("@b]")],
        )]
    );
}

#[test]
fn document_order_numbers_each_group() {
    let out = pe("@a and [@b]", cites());
    let nums: Vec<i32> = out
        .iter()
        .filter_map(|inline| match inline {
            Inline::Cite(citations, _) => citations.first().map(|c| c.note_num),
            _ => None,
        })
        .collect();
    assert_eq!(nums, vec![1, 2]);
}

#[test]
fn malformed_bracket_falls_back_to_inline_citations() {
    // a trailing empty segment voids the citation list; the bare `@a` becomes author-in-text
    assert_eq!(
        pe("[@a;]", cites()),
        vec![
            str("["),
            cite(
                vec![citation("a", vec![], vec![], CitationMode::AuthorInText, 1)],
                vec![str("@a")],
            ),
            str(";]"),
        ]
    );
}

#[test]
fn segment_without_a_key_is_not_a_citation_list() {
    // no `@` in the first segment voids the bracket; only the bare `@b` citation survives
    assert_eq!(
        pe("[no key; @b]", cites()),
        vec![
            str("[no"),
            Inline::Space,
            str("key;"),
            Inline::Space,
            cite(
                vec![citation("b", vec![], vec![], CitationMode::AuthorInText, 1)],
                vec![str("@b")],
            ),
            str("]"),
        ]
    );
}

#[test]
fn key_charset_keeps_internal_punctuation() {
    // Internal `_ : - . /` belong to a key only when more key characters follow.
    assert_eq!(
        pe("[@foo_bar:baz-qux.v/1]", cites()),
        vec![cite(
            vec![citation(
                "foo_bar:baz-qux.v/1",
                vec![],
                vec![],
                CitationMode::NormalCitation,
                1,
            )],
            vec![str("[@foo_bar:baz-qux.v/1]")],
        )]
    );
    // A trailing `-` is not part of the key; it falls to the suffix.
    assert_eq!(
        pe("[@a-]", cites()),
        vec![cite(
            vec![citation(
                "a",
                vec![],
                vec![str("-")],
                CitationMode::NormalCitation,
                1
            )],
            vec![str("[@a-]")],
        )]
    );
}

#[test]
fn citations_off_leaves_the_syntax_literal() {
    assert_eq!(
        pe("See [@a] and @b.", no_ext()),
        vec![
            str("See"),
            Inline::Space,
            str("[@a]"),
            Inline::Space,
            str("and"),
            Inline::Space,
            str("@b."),
        ]
    );
}

#[test]
fn escaped_at_is_not_a_citation() {
    assert_eq!(pe(r"[\@a]", cites()), vec![str("[@a]")]);
}

#[test]
fn citation_does_not_steal_a_link() {
    // An explicit link target wins; the key inside becomes a bare citation in the link text.
    assert_eq!(
        pe("[@a](http://x.com)", cites()),
        vec![link(
            vec![cite(
                vec![citation("a", vec![], vec![], CitationMode::AuthorInText, 1)],
                vec![str("@a")],
            )],
            "http://x.com",
        )]
    );
}

#[test]
fn heading_content_is_context_independent_gates_on_ref_trigger_chars() {
    assert!(heading_content_is_context_independent("Installation"));
    assert!(heading_content_is_context_independent("API reference"));
    assert!(!heading_content_is_context_independent("About @doe99"));
    assert!(!heading_content_is_context_independent("Title[^1]"));
    assert!(!heading_content_is_context_independent("See [spec]"));
}

#[test]
fn a_context_independent_heading_is_parsed_once_and_reused_by_the_body_pass() {
    let ir = vec![IrBlock::Heading(1, "Installation".to_owned())];
    let mut refs = empty_refs();
    let mut cache: HeaderParseCache = BTreeMap::new();
    let mut numbering = HeaderNumbering::new(no_ext(), false);
    gather_headers(
        &ir,
        &mut refs,
        no_notes(),
        no_ext(),
        &mut numbering,
        &mut cache,
    );

    let queued = cache
        .get("Installation")
        .expect("the pre-pass should have cached the heading's parse");
    assert_eq!(queued.len(), 1);

    let heading = ir.first().expect("one heading in the IR");
    let mut out = Vec::new();
    resolve_block(heading, &refs, no_notes(), no_ext(), &mut cache, &mut out);

    // The body pass popped the pre-pass's parse instead of running the inline scan again.
    assert!(cache.get("Installation").is_none_or(VecDeque::is_empty));
    assert_eq!(
        out,
        vec![Block::Header(1, Box::default(), p("Installation"))]
    );
}

#[test]
fn a_second_identical_heading_pops_its_own_queued_parse() {
    let ir = vec![
        IrBlock::Heading(1, "Dup".to_owned()),
        IrBlock::Heading(1, "Dup".to_owned()),
    ];
    let mut refs = empty_refs();
    let mut cache: HeaderParseCache = BTreeMap::new();
    let mut numbering = HeaderNumbering::new(no_ext(), false);
    gather_headers(
        &ir,
        &mut refs,
        no_notes(),
        no_ext(),
        &mut numbering,
        &mut cache,
    );
    assert_eq!(cache.get("Dup").map(VecDeque::len), Some(2));

    let mut out = Vec::new();
    for block in &ir {
        resolve_block(block, &refs, no_notes(), no_ext(), &mut cache, &mut out);
    }

    assert!(cache.get("Dup").is_none_or(VecDeque::is_empty));
    assert_eq!(
        out,
        vec![
            Block::Header(1, Box::default(), p("Dup")),
            Block::Header(1, Box::default(), p("Dup")),
        ]
    );
}
