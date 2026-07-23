use super::super::emphasis::{delimiter_literal, match_use_count};
use super::super::resolve::{
    TASK_CHECKED, TASK_UNCHECKED, split_header_attr, task_marker_replacement,
};
use super::{
    emoji, flanking, fold_dash_run_thirds, fold_ellipsis_run, interesting_chars,
    parse_meta_inlines, quote_flanking,
};
use carta_ast::{Attr, Inline, Target};
use carta_core::{Extension, Extensions};

fn is_interesting(table: &[bool; 128], ch: char) -> bool {
    usize::try_from(u32::from(ch))
        .ok()
        .and_then(|code| table.get(code))
        .copied()
        .unwrap_or(false)
}

#[test]
fn interesting_set_tracks_active_extensions() {
    let base = interesting_chars(Extensions::empty());
    assert!(is_interesting(&base, '*'));
    assert!(is_interesting(&base, '['));
    assert!(is_interesting(&base, '\n'));
    assert!(!is_interesting(&base, 'q'));
    assert!(!is_interesting(&base, '-'));
    assert!(!is_interesting(&base, ':'));
    assert!(!is_interesting(&base, 'é'));

    let gated = interesting_chars(exts(&[Extension::Smart, Extension::Emoji]));
    assert!(is_interesting(&gated, '-'));
    assert!(is_interesting(&gated, ':'));
    assert!(is_interesting(&gated, '*'));
    assert!(!is_interesting(&gated, 'q'));
    assert!(!is_interesting(&gated, 'é'));
}

fn exts(list: &[Extension]) -> Extensions {
    Extensions::from_list(list)
}

fn emoji_span(name: &str, text: &str) -> Inline {
    Inline::Span(
        Box::new(Attr {
            id: carta_ast::Text::default(),
            classes: vec!["emoji".to_owned().into()],
            attributes: vec![("data-emoji".to_owned().into(), name.to_owned().into())],
        }),
        vec![Inline::Str(text.to_owned().into())],
    )
}

#[test]
fn emoji_table_is_sorted_for_binary_search() {
    let on = exts(&[Extension::Emoji]);
    // A misordered table would make some entry unreachable by binary search.
    assert_eq!(emoji::lookup("smile"), Some("\u{1f604}"));
    assert_eq!(emoji::lookup("+1"), Some("\u{1f44d}"));
    assert_eq!(emoji::lookup("-1"), Some("\u{1f44e}"));
    // The multi-codepoint heart keeps its variation selector.
    assert_eq!(emoji::lookup("heart"), Some("\u{2764}\u{fe0f}"));
    assert_eq!(emoji::lookup("not_an_emoji_name"), None);
    // Parsing round-trips through the table for a representative name.
    assert_eq!(
        parse_meta_inlines(":rocket:", on, false),
        vec![emoji_span("rocket", "\u{1f680}")]
    );
}

#[test]
fn emoji_resolves_known_shortcodes() {
    let on = exts(&[Extension::Emoji]);
    assert_eq!(
        parse_meta_inlines(":smile:", on, false),
        vec![emoji_span("smile", "\u{1f604}")]
    );
    // A shortcode whose name carries `+`/`-` still resolves.
    assert_eq!(
        parse_meta_inlines(":+1:", on, false),
        vec![emoji_span("+1", "\u{1f44d}")]
    );
}

#[test]
fn emoji_unknown_name_stays_literal() {
    let on = exts(&[Extension::Emoji]);
    assert_eq!(
        parse_meta_inlines(":unknown_xyz:", on, false),
        vec![Inline::Str(":unknown_xyz:".to_owned().into())]
    );
    // An empty `::` is not a shortcode.
    assert_eq!(
        parse_meta_inlines("::", on, false),
        vec![Inline::Str("::".to_owned().into())]
    );
}

#[test]
fn emoji_requires_extension() {
    let off = Extensions::empty();
    assert_eq!(
        parse_meta_inlines(":smile:", off, false),
        vec![Inline::Str(":smile:".to_owned().into())]
    );
}

fn mark_span(content: Vec<Inline>) -> Inline {
    Inline::Span(
        Box::new(Attr {
            id: carta_ast::Text::default(),
            classes: vec!["mark".to_owned().into()],
            attributes: Vec::new(),
        }),
        content,
    )
}

#[test]
fn mark_resolves_inside_link_label() {
    let on = exts(&[Extension::Mark]);
    assert_eq!(
        parse_meta_inlines("[==hi==](u)", on, false),
        vec![Inline::Link(
            Box::default(),
            vec![mark_span(vec![Inline::Str("hi".to_owned().into())])],
            Box::new(Target {
                url: "u".to_owned().into(),
                title: carta_ast::Text::default(),
            }),
        )]
    );
}

#[test]
fn mark_resolves_inside_bracketed_span_label() {
    let on = exts(&[Extension::Mark, Extension::BracketedSpans]);
    let span_attr = Attr {
        id: carta_ast::Text::default(),
        classes: vec!["x".to_owned().into()],
        attributes: Vec::new(),
    };
    assert_eq!(
        parse_meta_inlines("[a ==b== c]{.x}", on, false),
        vec![Inline::Span(
            Box::new(span_attr),
            vec![
                Inline::Str("a".to_owned().into()),
                Inline::Space,
                mark_span(vec![Inline::Str("b".to_owned().into())]),
                Inline::Space,
                Inline::Str("c".to_owned().into()),
            ],
        )]
    );
}

#[test]
fn mark_in_label_requires_extension() {
    let off = Extensions::empty();
    assert_eq!(
        parse_meta_inlines("[==hi==](u)", off, false),
        vec![Inline::Link(
            Box::default(),
            vec![Inline::Str("==hi==".to_owned().into())],
            Box::new(Target {
                url: "u".to_owned().into(),
                title: carta_ast::Text::default(),
            }),
        )]
    );
}

#[test]
fn header_attr_split_requires_extension_and_trailing_block() {
    let on = exts(&[Extension::HeaderAttributes]);
    // A trailing block separated by whitespace is the heading's attribute.
    let (content, attr) = split_header_attr("Title {#id .cls}", on);
    assert_eq!(content, "Title");
    assert_eq!(attr.id, "id");
    assert_eq!(attr.classes, ["cls"]);
    // A block glued to the preceding word belongs to that word, not the heading.
    assert_eq!(split_header_attr("Title{#id}", on).0, "Title{#id}");
    // An empty block is left in the text.
    assert_eq!(split_header_attr("Title {}", on).0, "Title {}");
    // Without the extension the text is untouched.
    let (content, attr) = split_header_attr("Title {#id}", Extensions::empty());
    assert_eq!(content, "Title {#id}");
    assert!(attr.id.is_empty());
}

#[test]
fn mmd_header_identifier_split() {
    let on = exts(&[Extension::MmdHeaderIdentifiers]);
    // A trailing `[id]` label is the identifier; the content keeps everything before it.
    let (content, attr) = split_header_attr("Heading [myid]", on);
    assert_eq!(content, "Heading");
    assert_eq!(attr.id, "myid");
    // The identifier is lowercased with all whitespace removed.
    assert_eq!(split_header_attr("Foo [My Id]", on).1.id, "myid");
    // No whitespace is required before the label.
    let (content, attr) = split_header_attr("Foo[b]", on);
    assert_eq!((content, attr.id.as_str()), ("Foo", "b"));
    // Only the last bracket group is the label; earlier ones stay in the content.
    let (content, attr) = split_header_attr("Foo [a] bar [b]", on);
    assert_eq!((content, attr.id.as_str()), ("Foo [a] bar", "b"));
    // A reference-link tail (label directly after another bracket group) is not an identifier.
    assert_eq!(split_header_attr("See [ref][myid]", on).1.id, "");
    assert_eq!(split_header_attr("Foo [a] [b]", on).1.id, "");
    // An empty label is stripped but leaves the identifier empty (falls to an automatic one).
    assert_eq!(
        split_header_attr("Heading []", on),
        ("Heading", Attr::default())
    );
    // Without the extension the label stays in the text.
    assert_eq!(
        split_header_attr("Heading [myid]", Extensions::empty()).0,
        "Heading [myid]"
    );
}

#[test]
fn subscript_superscript_flanking_anchors_only_on_whitespace() {
    // Opens unless whitespace follows, closes unless whitespace precedes; no `*`/`_` punctuation sub-clauses.
    for ch in [b'~', b'^'] {
        assert_eq!(flanking(ch, None, Some('a')), (true, false));
        assert_eq!(flanking(ch, Some('a'), None), (false, true));
        assert_eq!(flanking(ch, Some('.'), Some('a')), (true, true));
        assert_eq!(flanking(ch, Some('a'), Some('!')), (true, true));
        assert_eq!(flanking(ch, Some(' '), Some('a')), (true, false));
        assert_eq!(flanking(ch, Some('a'), Some(' ')), (false, true));
    }
}

#[test]
fn asterisk_flanking_keeps_full_rules() {
    // `*` opener followed by punctuation and preceded by a letter is not left-flanking.
    assert_eq!(flanking(b'*', Some('a'), Some('!')), (false, true));
    // `_` keeps its intraword restriction: between two letters it can neither open nor close.
    assert_eq!(flanking(b'_', Some('a'), Some('b')), (false, false));
}

#[test]
fn use_count_maps_tilde_by_enabled_extension() {
    let strike = exts(&[Extension::Strikeout]);
    let sub = exts(&[Extension::Subscript]);
    let both = exts(&[Extension::Strikeout, Extension::Subscript]);

    // Two-on-two is a strikeout only when strikeout is on; otherwise it falls back to subscript.
    assert_eq!(match_use_count(2, 2, b'~', strike), Some(2));
    assert_eq!(match_use_count(2, 2, b'~', sub), Some(1));
    assert_eq!(match_use_count(2, 2, b'~', both), Some(2));
    // A length-one run can only be a subscript.
    assert_eq!(match_use_count(1, 2, b'~', strike), None);
    assert_eq!(match_use_count(1, 2, b'~', sub), Some(1));
    // With neither extension a tilde is inert.
    assert_eq!(match_use_count(2, 2, b'~', Extensions::empty()), None);
}

#[test]
fn use_count_for_caret_and_emphasis() {
    assert_eq!(match_use_count(1, 1, b'^', Extensions::empty()), Some(1));
    assert_eq!(match_use_count(3, 3, b'^', Extensions::empty()), Some(1));
    assert_eq!(match_use_count(2, 2, b'*', Extensions::empty()), Some(2));
    assert_eq!(match_use_count(1, 2, b'_', Extensions::empty()), Some(1));
}

#[test]
fn dash_runs_fold_em_heavy() {
    let em = '\u{2014}';
    let en = '\u{2013}';
    // Multiples of three are all em; even lengths are all en.
    assert_eq!(fold_dash_run_thirds(2), en.to_string());
    assert_eq!(fold_dash_run_thirds(3), em.to_string());
    assert_eq!(fold_dash_run_thirds(4), format!("{en}{en}"));
    assert_eq!(fold_dash_run_thirds(6), format!("{em}{em}"));
    // Odd lengths that are not multiples of three are em-heavy with a one- or two-en tail.
    assert_eq!(fold_dash_run_thirds(5), format!("{em}{en}"));
    assert_eq!(fold_dash_run_thirds(7), format!("{em}{en}{en}"));
    assert_eq!(fold_dash_run_thirds(11), format!("{em}{em}{em}{en}"));
    assert_eq!(fold_dash_run_thirds(13), format!("{em}{em}{em}{en}{en}"));
    assert_eq!(
        fold_dash_run_thirds(17),
        format!("{em}{em}{em}{em}{em}{en}")
    );
    // Em counts three hyphens, en two, so widths sum back to the run length with none left over.
    for len in 2..=40 {
        let folded = fold_dash_run_thirds(len);
        let width: usize = folded.chars().map(|c| if c == em { 3 } else { 2 }).sum();
        assert_eq!(width, len, "len={len} folded={folded}");
    }
}

#[test]
fn ellipsis_runs_fold_in_threes() {
    assert_eq!(fold_ellipsis_run(0), "");
    assert_eq!(fold_ellipsis_run(1), ".");
    assert_eq!(fold_ellipsis_run(2), "..");
    assert_eq!(fold_ellipsis_run(3), "\u{2026}");
    assert_eq!(fold_ellipsis_run(4), "\u{2026}.");
    assert_eq!(fold_ellipsis_run(7), "\u{2026}\u{2026}.");
}

#[test]
fn unmatched_smart_quotes_become_curly() {
    // A single quote that never pairs closes (’); an unmatched double quote opens (“).
    assert_eq!(delimiter_literal(b'\'', 1), "\u{2019}");
    assert_eq!(delimiter_literal(b'"', 1), "\u{201c}");
    assert_eq!(delimiter_literal(b'\'', 2), "\u{2019}\u{2019}");
    // Other delimiters revert to their own character.
    assert_eq!(delimiter_literal(b'*', 3), "***");
}

#[test]
fn quote_flanking_blocks_intraword_pairing() {
    // A quote between alphanumerics can neither open nor close, so contractions stay apostrophes.
    assert_eq!(quote_flanking(b'\'', Some('n'), Some('t')), (false, false));
    // Whitespace-anchored quotes open on the left edge and close on the right.
    assert_eq!(quote_flanking(b'"', Some(' '), Some('a')), (true, false));
    assert_eq!(quote_flanking(b'"', Some('a'), Some(' ')), (false, true));
    // A quote hugging punctuation can both open and close.
    assert_eq!(quote_flanking(b'\'', Some('('), Some('a')), (true, false));
}

#[test]
fn task_marker_replacement_recognizes_only_bounded_markers() {
    assert_eq!(
        task_marker_replacement("[ ] todo").as_deref(),
        Some(&*format!("{TASK_UNCHECKED} todo"))
    );
    assert_eq!(
        task_marker_replacement("[x] done").as_deref(),
        Some(&*format!("{TASK_CHECKED} done"))
    );
    assert_eq!(
        task_marker_replacement("[X]").as_deref(),
        Some(TASK_CHECKED)
    );
    // A marker glued to following text is not a task marker.
    assert_eq!(task_marker_replacement("[ ]todo"), None);
    // Unknown fill characters are not markers.
    assert_eq!(task_marker_replacement("[y] no"), None);
    assert_eq!(task_marker_replacement("plain"), None);
}

fn str_node(text: &str) -> Inline {
    Inline::Str(text.to_owned().into())
}

fn emph(content: Vec<Inline>) -> Inline {
    Inline::Emph(content)
}

fn strong(content: Vec<Inline>) -> Inline {
    Inline::Strong(content)
}

fn commonmark(text: &str) -> Vec<Inline> {
    parse_meta_inlines(text, Extensions::empty(), false)
}

#[test]
fn triple_run_nests_strong_inside_emph() {
    assert_eq!(
        commonmark("***a***"),
        vec![emph(vec![strong(vec![str_node("a")])])]
    );
}

#[test]
fn triple_run_markdown_dialect_nests_emph_inside_strong() {
    assert_eq!(
        parse_meta_inlines("***a***", Extensions::empty(), true),
        vec![strong(vec![emph(vec![str_node("a")])])]
    );
}

#[test]
fn strong_wraps_inner_emph_and_text() {
    assert_eq!(
        commonmark("**a *b* c**"),
        vec![strong(vec![
            str_node("a"),
            Inline::Space,
            emph(vec![str_node("b")]),
            Inline::Space,
            str_node("c"),
        ])]
    );
}

#[test]
fn emph_then_trailing_strong() {
    assert_eq!(
        commonmark("*a**b***"),
        vec![emph(vec![str_node("a"), strong(vec![str_node("b")])])]
    );
}

#[test]
fn mixed_underscore_inside_asterisk() {
    assert_eq!(
        commonmark("*a _b_ a*"),
        vec![emph(vec![
            str_node("a"),
            Inline::Space,
            emph(vec![str_node("b")]),
            Inline::Space,
            str_node("a"),
        ])]
    );
}

#[test]
fn unmatched_leading_run_stays_literal() {
    assert_eq!(
        commonmark("***a*"),
        vec![str_node("**"), emph(vec![str_node("a")])]
    );
}

#[test]
fn adjacent_emphasis_chain() {
    assert_eq!(
        commonmark("*a*a*a*"),
        vec![
            emph(vec![str_node("a")]),
            str_node("a"),
            emph(vec![str_node("a")]),
        ]
    );
}

#[test]
fn adjacent_emphasis_chain_ten() {
    let mut expected = Vec::new();
    for _ in 0..5 {
        expected.push(emph(vec![str_node("a")]));
        expected.push(str_node("a"));
    }
    assert_eq!(commonmark("*a*a*a*a*a*a*a*a*a*a"), expected);
}

#[test]
fn strikeout_and_mark_side_by_side() {
    let ext = exts(&[Extension::Strikeout, Extension::Mark]);
    assert_eq!(
        parse_meta_inlines("~~x~~ and ==y==", ext, false),
        vec![
            Inline::Strikeout(vec![str_node("x")]),
            Inline::Space,
            str_node("and"),
            Inline::Space,
            mark_span(vec![str_node("y")]),
        ]
    );
}

#[test]
fn subscript_run() {
    let ext = exts(&[Extension::Subscript]);
    assert_eq!(
        parse_meta_inlines("H~2~O", ext, false),
        vec![
            str_node("H"),
            Inline::Subscript(vec![str_node("2")]),
            str_node("O"),
        ]
    );
}

#[test]
fn mark_wraps_inner_strong() {
    let ext = exts(&[Extension::Mark]);
    assert_eq!(
        parse_meta_inlines("==a **b** c==", ext, false),
        vec![mark_span(vec![
            str_node("a"),
            Inline::Space,
            strong(vec![str_node("b")]),
            Inline::Space,
            str_node("c"),
        ])]
    );
}
