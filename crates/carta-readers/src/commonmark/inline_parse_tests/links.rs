//! Link and image inline-parse tests.

use super::*;

#[test]
fn wiki_links_select_the_pipe_direction_and_keep_labels_literal() {
    for (extension, source) in [
        (Extension::WikilinksTitleAfterPipe, "[[Page Name|*label*]]"),
        (Extension::WikilinksTitleBeforePipe, "[[*label*|Page Name]]"),
    ] {
        let inlines = pm(source, exts(&[extension]));
        let [Inline::Link(attributes, label, target)] = inlines.as_slice() else {
            panic!("{inlines:?}");
        };
        assert_eq!(attributes.classes, ["wikilink"]);
        assert_eq!(label, &[str("*label*")]);
        assert_eq!(target.url, "Page Name");
    }
    assert_eq!(pm("[[Page|label]]", no_ext()), vec![str("[[Page|label]]")]);
}

#[test]
fn wiki_images_decode_label_entities_and_keep_targets_verbatim() {
    let inlines = pm(
        "![[a&amp;b|x&amp;y]]",
        exts(&[Extension::WikilinksTitleAfterPipe]),
    );
    let [Inline::Image(attributes, label, target)] = inlines.as_slice() else {
        panic!("{inlines:?}");
    };
    assert_eq!(attributes.classes, ["wikilink"]);
    assert_eq!(label, &[str("x&y")]);
    assert_eq!(target.url, "a&amp;b");
    assert_eq!(
        pm(
            "[[p|x]]{#id}",
            exts(&[
                Extension::WikilinksTitleAfterPipe,
                Extension::BracketedSpans
            ])
        ),
        vec![span(attr("id", &[], &[]), vec![str("[p|x]")])]
    );
}

#[test]
fn failed_wiki_openers_keep_links_on_later_lines() {
    let prefix = "[[a".repeat(8000);
    let source = format!("{prefix}\n[[Page]]");
    let inlines = pm(&source, exts(&[Extension::WikilinksTitleAfterPipe]));
    assert!(
        matches!(inlines.as_slice(), [Inline::Str(text), Inline::SoftBreak, Inline::Link(_, _, target)] if text.as_str() == prefix && target.url == "Page")
    );
}

#[test]
fn commonmark_wiki_links_trim_edges_decode_targets_and_keep_soft_breaks() {
    let extensions = exts(&[Extension::WikilinksTitleAfterPipe]);
    let inlines = pe("[[ a&amp;b | c d ]]", extensions);
    let [Inline::Link(_, content, target)] = inlines.as_slice() else {
        panic!("{inlines:?}");
    };
    assert_eq!(target.url, "a&b");
    assert_eq!(content, &[str("c"), Inline::Space, str("d")]);
    let inlines = pe("[[a\nb]]", extensions);
    let [Inline::Link(_, content, target)] = inlines.as_slice() else {
        panic!("{inlines:?}");
    };
    assert_eq!(target.url, "a\nb");
    assert_eq!(content, &[str("a"), Inline::SoftBreak, str("b")]);
    assert_eq!(pe("[[a]b]]", extensions), vec![str("[[a]b]]")]);
    assert!(
        matches!(pe("![[Page]]", extensions).as_slice(), [Inline::Str(text), Inline::Link(..)] if text == "!")
    );
}

#[test]
fn commonmark_wiki_links_keep_attached_attributes() {
    let inlines = pe(
        "[[Page|label]]{#id .x}",
        exts(&[Extension::WikilinksTitleAfterPipe, Extension::Attributes]),
    );
    let [Inline::Link(attributes, _, _)] = inlines.as_slice() else {
        panic!("{inlines:?}");
    };
    assert_eq!(attributes.id, "id");
    assert_eq!(attributes.classes, ["x", "wikilink"]);
}

#[test]
fn wiki_links_leave_new_brackets_active_after_older_openers_close() {
    let inlines = pm(
        "[[Page]] [outer [inner](one)](two) [later](three)",
        exts(&[Extension::WikilinksTitleAfterPipe]),
    );
    let targets: Vec<&str> = inlines
        .iter()
        .filter_map(|inline| match inline {
            Inline::Link(_, _, target) => Some(target.url.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(targets, ["Page", "one", "three"]);
    assert!(inlines.contains(&str("](two)")));
}

// Links and images

#[test]
fn inline_link_and_image() {
    assert_eq!(p("[a](u)"), vec![link(vec![str("a")], "u")]);
    assert_eq!(p("![i](u)"), vec![image(vec![str("i")], "u")]);
}

#[test]
fn unmatched_image_opener_keeps_its_bang() {
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
    let budget = Cell::new(usize::MAX);
    let notes = RefContext {
        defined: &defined,
        by_id: &by_id,
        in_definition: false,
        markdown: false,
        examples: &examples,
        cite_count: &cite,
        cite_budget: &budget,
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
    assert_eq!(
        parse_inlines("[text] [ref]", &refs, no_notes(), ext),
        vec![link(vec![str("text")], "http://r")]
    );
    assert_eq!(
        parse_inlines("[text]\n[ref]", &refs, no_notes(), ext),
        vec![link(vec![str("text")], "http://r")]
    );
    assert_eq!(
        parse_inlines("[text] []", &refs, no_notes(), ext),
        vec![link(vec![str("text")], "http://t")]
    );
    let only_text = ref_map(&[("text", "http://t")]);
    assert_eq!(
        parse_inlines("[text] [ref]", &only_text, no_notes(), ext),
        vec![str("[text]"), Inline::Space, str("[ref]")]
    );
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
    assert_eq!(p("[[a]](u)"), vec![link(vec![str("[a]")], "u")]);
}

#[test]
fn unmatched_brackets_are_literal() {
    assert_eq!(p("]]]"), vec![str("]]]")]);
}

#[test]
fn link_suppresses_earlier_bracket_openers() {
    // An inner link deactivates the outer opener because links cannot contain links.
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
